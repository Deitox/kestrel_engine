use super::*;
use crate::assets::AssetManager;
use crate::ecs::systems::{initialize_animation_phase, AnimationTime, TimeDelta};
use crate::events::{EventBus, GameEvent};
use crate::mesh_registry::MeshRegistry;
use crate::scene::{
    ColliderData, ColorData, MeshData, MeshLightingData, OrbitControllerData, ParticleEmitterData, Scene,
    SceneDependencies, SceneEntity, SceneEntityId, SpriteAnimationData, SpriteData, Transform3DData,
    TransformData,
};
use anyhow::{anyhow, Result};
use bevy_ecs::prelude::{Entity, Schedule, With, World};
use glam::{EulerRot, Mat4, Quat, Vec2, Vec3, Vec4};
use rand::Rng;
use rapier2d::prelude::{Rotation, Vector};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

pub struct EmitterSnapshot {
    pub rate: f32,
    pub spread: f32,
    pub speed: f32,
    pub lifetime: f32,
    pub start_color: Vec4,
    pub end_color: Vec4,
    pub start_size: f32,
    pub end_size: f32,
}

// ---------- World container ----------
pub struct EcsWorld {
    pub world: World,
    schedule_var: Schedule,
    schedule_fixed: Schedule,
}

impl EcsWorld {
    pub fn new() -> Self {
        let mut world = World::new();
        world.insert_resource(TimeDelta(0.0));
        world.insert_resource(AnimationTime::default());
        world.insert_resource(SpatialHash::new(0.25));
        world.insert_resource(SpatialQuadtree::new(6, 8));
        world.insert_resource(SpatialIndexConfig::default());
        world.insert_resource(SpatialMetrics::default());
        world.insert_resource(ParticleContacts::default());
        world.insert_resource(ParticleCaps::default());
        let world_bounds =
            WorldBounds { min: Vec2::new(-1.4, -1.0), max: Vec2::new(1.4, 1.0), thickness: 0.05 };
        world.insert_resource(world_bounds);
        let physics_params = PhysicsParams { gravity: Vec2::new(0.0, -0.6), linear_damping: 0.3 };
        world.insert_resource(physics_params);
        let boundary_entity = world.spawn_empty().id();
        world.insert_resource(RapierState::new(&physics_params, &world_bounds, boundary_entity));
        world.insert_resource(EventBus::default());
        world.insert_resource(TransformPropagationScratch::default());
        world.insert_resource(SystemProfiler::new());

        let mut schedule_var = Schedule::default();
        schedule_var.add_systems((
            sys_apply_spin,
            sys_propagate_scene_transforms,
            sys_sync_world3d,
            sys_update_emitters,
            sys_update_particles,
            sys_drive_transform_clips,
            sys_drive_skeletal_clips,
            sys_drive_sprite_animations,
        ));

        let mut schedule_fixed = Schedule::default();
        schedule_fixed.add_systems((
            sys_solve_forces,
            sys_integrate_positions,
            sys_drive_orbits,
            sys_step_rapier,
            sys_sync_from_rapier,
            sys_world_bounds_bounce,
            sys_build_spatial_hash,
            sys_collide_spatial,
        ));

        Self { world, schedule_var, schedule_fixed }
    }

    fn emit(&mut self, event: GameEvent) {
        self.world.resource_mut::<EventBus>().push(event);
    }

    pub fn drain_events(&mut self) -> Vec<GameEvent> {
        self.world.resource_mut::<EventBus>().drain()
    }

    pub fn push_event(&mut self, event: GameEvent) {
        self.emit(event);
    }

    fn apply_sprite_snapshot(&mut self, entity: Entity, snapshot: Option<(Arc<str>, u16, [f32; 4])>) {
        if let Some((region, region_id, uv)) = snapshot {
            if let Some(mut sprite) = self.world.get_mut::<Sprite>(entity) {
                sprite.region = region;
                sprite.region_id = region_id;
                sprite.uv = uv;
            }
        }
    }

    fn current_frame_snapshot(animation: &SpriteAnimation) -> Option<(Arc<str>, u16, [f32; 4])> {
        animation.current_frame().map(|frame| (frame.region.clone(), frame.region_id, frame.uv))
    }

    pub fn spawn_demo_scene(&mut self) -> Entity {
        let root = self
            .world
            .spawn((
                Transform { translation: Vec2::ZERO, rotation: 0.0, scale: Vec2::splat(0.8) },
                WorldTransform::default(),
                Spin { speed: 1.2 },
                Sprite::uninitialized(Arc::from("main"), Arc::from("checker")),
                Tint(Vec4::new(1.0, 1.0, 1.0, 0.2)),
            ))
            .id();
        self.ensure_scene_entity_tag(root);
        self.ensure_scene_entity_tag(root);
        self.emit(GameEvent::SpriteSpawned {
            entity: root,
            atlas: "main".to_string(),
            region: "checker".to_string(),
        });

        let orbit_center = Vec2::ZERO;
        let orbit_speed_a = 0.9;
        let orbit_speed_b = 0.95;
        let orbit_speed_c = 1.05;

        let translation_a = Vec2::new(-0.9, 0.0);
        let half_a = Vec2::splat(0.35);
        let velocity_a = Vec2::new(-translation_a.y, translation_a.x) * orbit_speed_a;
        let (body_a, collider_a) = {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.spawn_dynamic_body(translation_a, half_a, 1.0, velocity_a)
        };
        let a = self
            .world
            .spawn((
                Transform { translation: translation_a, rotation: 0.0, scale: Vec2::splat(0.7) },
                WorldTransform::default(),
                Sprite::uninitialized(Arc::from("main"), Arc::from("checker")),
                Aabb { half: half_a },
                Velocity(velocity_a),
                Force::default(),
                Mass(1.0),
                RapierBody { handle: body_a },
                RapierCollider { handle: collider_a },
                OrbitController { center: orbit_center, angular_speed: orbit_speed_a },
            ))
            .id();
        self.ensure_scene_entity_tag(a);
        self.ensure_scene_entity_tag(a);
        {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.register_collider_entity(collider_a, a);
        }
        self.emit(GameEvent::SpriteSpawned {
            entity: a,
            atlas: "main".to_string(),
            region: "checker".to_string(),
        });

        let translation_b = Vec2::new(0.9, 0.0);
        let half_b = Vec2::splat(0.30);
        let velocity_b = Vec2::new(-translation_b.y, translation_b.x) * orbit_speed_b;
        let (body_b, collider_b) = {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.spawn_dynamic_body(translation_b, half_b, 1.0, velocity_b)
        };
        let b = self
            .world
            .spawn((
                Transform { translation: translation_b, rotation: 0.0, scale: Vec2::splat(0.6) },
                WorldTransform::default(),
                Sprite::uninitialized(Arc::from("main"), Arc::from("redorb")),
                Aabb { half: half_b },
                Velocity(velocity_b),
                Force::default(),
                Mass(1.0),
                RapierBody { handle: body_b },
                RapierCollider { handle: collider_b },
                OrbitController { center: orbit_center, angular_speed: orbit_speed_b },
            ))
            .id();
        self.ensure_scene_entity_tag(b);
        self.ensure_scene_entity_tag(b);
        {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.register_collider_entity(collider_b, b);
        }
        self.emit(GameEvent::SpriteSpawned {
            entity: b,
            atlas: "main".to_string(),
            region: "redorb".to_string(),
        });

        let translation_c = Vec2::new(0.0, 0.9);
        let half_c = Vec2::splat(0.25);
        let velocity_c = Vec2::new(-translation_c.y, translation_c.x) * orbit_speed_c;
        let (body_c, collider_c) = {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.spawn_dynamic_body(translation_c, half_c, 1.0, velocity_c)
        };
        let c = self
            .world
            .spawn((
                Transform { translation: translation_c, rotation: 0.0, scale: Vec2::splat(0.5) },
                WorldTransform::default(),
                Sprite::uninitialized(Arc::from("main"), Arc::from("bluebox")),
                Aabb { half: half_c },
                Velocity(velocity_c),
                Force::default(),
                Mass(1.0),
                RapierBody { handle: body_c },
                RapierCollider { handle: collider_c },
                OrbitController { center: orbit_center, angular_speed: orbit_speed_c },
            ))
            .id();
        self.ensure_scene_entity_tag(c);
        self.ensure_scene_entity_tag(c);
        {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.register_collider_entity(collider_c, c);
        }
        self.emit(GameEvent::SpriteSpawned {
            entity: c,
            atlas: "main".to_string(),
            region: "bluebox".to_string(),
        });

        let emitter = self.spawn_particle_emitter(
            Vec2::new(0.0, 0.0),
            35.0,
            std::f32::consts::PI / 3.0,
            0.8,
            1.2,
            Vec4::new(1.0, 0.8, 0.2, 0.8),
            Vec4::new(1.0, 0.2, 0.2, 0.0),
            0.18,
            0.05,
        );
        emitter
    }

    pub fn spawn_burst(&mut self, _assets: &AssetManager, count: usize) {
        let regions = ["checker", "redorb", "bluebox", "green"];
        let mut rng = rand::thread_rng();
        for _ in 0..count {
            let rname = regions[rng.gen_range(0..regions.len())];
            let pos = Vec2::new(rng.gen_range(-1.2..1.2), rng.gen_range(-0.9..0.9));
            let vel = Vec2::new(rng.gen_range(-1.0..1.0), rng.gen_range(-1.0..1.0)) * 0.6;
            let scale = rng.gen_range(0.15..0.35);
            let half = Vec2::splat(scale * 0.5);
            let (body_handle, collider_handle) = {
                let mut rapier = self.world.resource_mut::<RapierState>();
                rapier.spawn_dynamic_body(pos, half, 0.8, vel)
            };
            let entity = self
                .world
                .spawn((
                    Transform { translation: pos, rotation: 0.0, scale: Vec2::splat(scale) },
                    WorldTransform::default(),
                    Sprite::uninitialized(Arc::from("main"), Arc::from(rname)),
                    Aabb { half },
                    Velocity(vel),
                    Force::default(),
                    Mass(0.8),
                    RapierBody { handle: body_handle },
                    RapierCollider { handle: collider_handle },
                ))
                .id();
            self.ensure_scene_entity_tag(entity);
            {
                let mut rapier = self.world.resource_mut::<RapierState>();
                rapier.register_collider_entity(collider_handle, entity);
            }
            self.emit(GameEvent::SpriteSpawned {
                entity,
                atlas: "main".to_string(),
                region: rname.to_string(),
            });
        }
    }

    pub fn spawn_particle_emitter(
        &mut self,
        position: Vec2,
        rate: f32,
        spread: f32,
        speed: f32,
        lifetime: f32,
        start_color: Vec4,
        end_color: Vec4,
        start_size: f32,
        end_size: f32,
    ) -> Entity {
        let entity = self
            .world
            .spawn((
                Transform { translation: position, rotation: 0.0, scale: Vec2::splat(start_size) },
                WorldTransform::default(),
                ParticleEmitter {
                    rate,
                    spread,
                    speed,
                    lifetime,
                    accumulator: 0.0,
                    start_color,
                    end_color,
                    start_size,
                    end_size,
                },
            ))
            .id();
        self.ensure_scene_entity_tag(entity);
        entity
    }

    pub fn set_emitter_rate(&mut self, entity: Entity, rate: f32) {
        if let Some(mut emitter) = self.world.get_mut::<ParticleEmitter>(entity) {
            emitter.rate = rate.max(0.0);
        }
    }

    pub fn set_emitter_colors(&mut self, entity: Entity, start: Vec4, end: Vec4) {
        if let Some(mut emitter) = self.world.get_mut::<ParticleEmitter>(entity) {
            emitter.start_color = start;
            emitter.end_color = end;
        }
    }

    pub fn set_emitter_sizes(&mut self, entity: Entity, start: f32, end: f32) {
        if let Some(mut emitter) = self.world.get_mut::<ParticleEmitter>(entity) {
            emitter.start_size = start.max(0.01);
            emitter.end_size = end.max(0.01);
        }
    }

    pub fn clear_particles(&mut self) {
        let mut particles = Vec::new();
        {
            let mut query = self.world.query_filtered::<Entity, With<Particle>>();
            for entity in query.iter(&self.world) {
                particles.push(entity);
            }
        }
        for entity in particles {
            let _ = self.world.despawn(entity);
        }
        let mut emitters = self.world.query::<&mut ParticleEmitter>();
        for mut emitter in emitters.iter_mut(&mut self.world) {
            emitter.accumulator = 0.0;
        }
        self.world.resource_mut::<ParticleContacts>().pairs.clear();
    }

    pub fn particle_budget_metrics(&mut self) -> ParticleBudgetMetrics {
        let caps = *self.world.resource::<ParticleCaps>();
        let mut particle_query = self.world.query::<&Particle>();
        let active_particles = particle_query.iter(&self.world).count() as u32;
        let mut emitter_query = self.world.query::<&ParticleEmitter>();
        let mut total_emitters = 0u32;
        let mut backlog_total = 0.0f32;
        let mut backlog_max = 0.0f32;
        for emitter in emitter_query.iter(&self.world) {
            total_emitters += 1;
            backlog_total += emitter.accumulator;
            backlog_max = backlog_max.max(emitter.accumulator);
        }
        let available_spawn = caps.max_total.saturating_sub(active_particles).min(caps.max_spawn_per_frame);
        ParticleBudgetMetrics {
            active_particles,
            available_spawn_this_frame: available_spawn,
            max_total: caps.max_total,
            max_spawn_per_frame: caps.max_spawn_per_frame,
            total_emitters,
            emitter_backlog_total: backlog_total,
            emitter_backlog_max_observed: backlog_max,
            emitter_backlog_limit: caps.max_emitter_backlog,
        }
    }

    pub fn clear_world(&mut self) {
        {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.clear_dynamic();
        }
        let boundary = {
            let rapier = self.world.resource::<RapierState>();
            rapier.boundary_entity()
        };
        let entities: Vec<Entity> =
            self.world.iter_entities().map(|e| e.id()).filter(|entity| *entity != boundary).collect();
        for entity in entities {
            let _ = self.world.despawn(entity);
        }
        self.world.resource_mut::<ParticleContacts>().pairs.clear();
    }

    pub fn set_particle_caps(&mut self, caps: ParticleCaps) {
        *self.world.resource_mut::<ParticleCaps>() = caps;
    }

    pub fn set_emitter_spread(&mut self, entity: Entity, spread: f32) {
        if let Some(mut emitter) = self.world.get_mut::<ParticleEmitter>(entity) {
            emitter.spread = spread.clamp(0.0, std::f32::consts::PI);
        }
    }

    pub fn set_emitter_speed(&mut self, entity: Entity, speed: f32) {
        if let Some(mut emitter) = self.world.get_mut::<ParticleEmitter>(entity) {
            emitter.speed = speed.max(0.0);
        }
    }

    pub fn set_emitter_lifetime(&mut self, entity: Entity, lifetime: f32) {
        if let Some(mut emitter) = self.world.get_mut::<ParticleEmitter>(entity) {
            emitter.lifetime = lifetime.max(0.05);
        }
    }

    pub fn update(&mut self, dt: f32) {
        self.world.resource_mut::<TimeDelta>().0 = dt;
        self.schedule_var.run(&mut self.world);
    }
    pub fn fixed_step(&mut self, dt: f32) {
        self.world.resource_mut::<TimeDelta>().0 = dt;
        self.schedule_fixed.run(&mut self.world);
    }
    pub fn adjust_root_spin(&mut self, delta: f32) {
        let mut q = self.world.query::<&mut Spin>();
        for mut s in q.iter_mut(&mut self.world) {
            s.speed += delta;
            break;
        }
    }
    pub fn spawn_scripted_sprite(
        &mut self,
        assets: &AssetManager,
        atlas: &str,
        region: &str,
        position: Vec2,
        scale: f32,
        velocity: Vec2,
    ) -> Result<Entity> {
        if scale <= 0.0 {
            return Err(anyhow!("Scale must be positive"));
        }
        let Some((region_name, info)) = assets.atlas_region_info(atlas, region) else {
            return Err(anyhow!("Region '{region}' not found in atlas '{atlas}'"));
        };
        let half = Vec2::splat(scale * 0.5);
        let (body_handle, collider_handle) = {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.spawn_dynamic_body(position, half, 1.0, velocity)
        };
        let entity = self
            .world
            .spawn((
                Transform { translation: position, rotation: 0.0, scale: Vec2::splat(scale) },
                WorldTransform::default(),
                Sprite {
                    atlas_key: Arc::from(atlas.to_string()),
                    region: Arc::clone(region_name),
                    region_id: info.id,
                    uv: info.uv,
                },
                Aabb { half },
                Velocity(velocity),
                Force::default(),
                Mass(1.0),
                RapierBody { handle: body_handle },
                RapierCollider { handle: collider_handle },
            ))
            .id();
        self.ensure_scene_entity_tag(entity);
        {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.register_collider_entity(collider_handle, entity);
        }
        self.emit(GameEvent::SpriteSpawned {
            entity,
            atlas: atlas.to_string(),
            region: region_name.as_ref().to_string(),
        });
        Ok(entity)
    }

    pub fn spawn_mesh_entity(&mut self, mesh_key: &str, translation: Vec3, scale: Vec3) -> Entity {
        let transform3d = Transform3D { translation, rotation: Quat::IDENTITY, scale };
        let world3d =
            WorldTransform3D(Mat4::from_scale_rotation_translation(scale, Quat::IDENTITY, translation));
        let entity = self
            .world
            .spawn((
                Transform::default(),
                WorldTransform::default(),
                transform3d,
                world3d,
                MeshRef { key: mesh_key.to_string() },
                MeshSurface::default(),
            ))
            .id();
        self.ensure_scene_entity_tag(entity);
        entity
    }
    pub fn set_velocity(&mut self, entity: Entity, velocity: Vec2) -> bool {
        let mut updated = false;
        {
            if let Some(mut vel) = self.world.get_mut::<Velocity>(entity) {
                vel.0 = velocity;
                updated = true;
            }
        }
        if let Some(handle) = self.world.get::<RapierBody>(entity).map(|b| b.handle) {
            let mut rapier = self.world.resource_mut::<RapierState>();
            if let Some(body) = rapier.body_mut(handle) {
                body.set_linvel(Vector::new(velocity.x, velocity.y), true);
            }
            updated = true;
        }
        updated
    }
    pub fn set_translation(&mut self, entity: Entity, translation: Vec2) -> bool {
        let mut changed = false;
        {
            if let Some(mut transform) = self.world.get_mut::<Transform>(entity) {
                transform.translation = translation;
                changed = true;
            }
        }
        if let Some(handle) = self.world.get::<RapierBody>(entity).map(|b| b.handle) {
            let mut rapier = self.world.resource_mut::<RapierState>();
            if let Some(body) = rapier.body_mut(handle) {
                body.set_translation(Vector::new(translation.x, translation.y), true);
            }
            changed = true;
        }
        changed
    }
    pub fn set_rotation(&mut self, entity: Entity, rotation: f32) -> bool {
        let mut changed = false;
        {
            if let Some(mut transform) = self.world.get_mut::<Transform>(entity) {
                transform.rotation = rotation;
                changed = true;
            }
        }
        if let Some(handle) = self.world.get::<RapierBody>(entity).map(|b| b.handle) {
            let mut rapier = self.world.resource_mut::<RapierState>();
            if let Some(body) = rapier.body_mut(handle) {
                body.set_rotation(Rotation::new(rotation), true);
            }
            changed = true;
        }
        changed
    }
    pub fn set_scale(&mut self, entity: Entity, scale: Vec2) -> bool {
        if scale.x.abs() < f32::EPSILON || scale.y.abs() < f32::EPSILON {
            return false;
        }
        let mut changed = false;
        if let Some(mut transform) = self.world.get_mut::<Transform>(entity) {
            transform.scale = scale;
            changed = true;
        }
        let half = Vec2::new(scale.x.abs(), scale.y.abs()) * 0.5;
        let previous_half = self.world.get::<Aabb>(entity).map(|a| a.half);
        if let Some(mut aabb) = self.world.get_mut::<Aabb>(entity) {
            aabb.half = half;
            changed = true;
        }
        let mut new_mass_value = None;
        if let Some(mut mass) = self.world.get_mut::<Mass>(entity) {
            let prev = previous_half.unwrap_or(half);
            let old_area = (prev.x.max(0.01) * 2.0) * (prev.y.max(0.01) * 2.0);
            let new_area = (half.x.max(0.01) * 2.0) * (half.y.max(0.01) * 2.0);
            if old_area > 0.0 {
                mass.0 = (mass.0 * (new_area / old_area)).max(0.01);
                new_mass_value = Some(mass.0);
                changed = true;
            }
        }
        let collider_handle = self.world.get::<RapierCollider>(entity).map(|c| c.handle);
        let body_handle = self.world.get::<RapierBody>(entity).map(|b| b.handle);
        if collider_handle.is_some() || (body_handle.is_some() && new_mass_value.is_some()) {
            let mut rapier = self.world.resource_mut::<RapierState>();
            if let Some(handle) = collider_handle {
                rapier.resize_collider(handle, half);
            }
            if let (Some(handle), Some(mass_value)) = (body_handle, new_mass_value) {
                rapier.set_body_mass(handle, mass_value);
            }
            changed = true;
        }
        changed
    }
    pub fn set_tint(&mut self, entity: Entity, color: Option<Vec4>) -> bool {
        match color {
            Some(color) => {
                if let Some(mut tint) = self.world.get_mut::<Tint>(entity) {
                    tint.0 = color;
                    true
                } else {
                    self.world.entity_mut(entity).insert(Tint(color));
                    true
                }
            }
            None => {
                if self.world.get::<Tint>(entity).is_some() {
                    self.world.entity_mut(entity).remove::<Tint>();
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn set_transform_clip(&mut self, entity: Entity, assets: &AssetManager, clip_key: &str) -> bool {
        let clip_data = match assets.clip(clip_key) {
            Some(clip) => clip.clone(),
            None => return false,
        };
        let clip_arc = Arc::new(clip_data);
        let clip_name: Arc<str> = Arc::from(clip_key.to_string());
        let sample = {
            if let Some(mut instance) = self.world.get_mut::<ClipInstance>(entity) {
                instance.replace_clip(Arc::clone(&clip_name), Arc::clone(&clip_arc));
                let sample = instance.sample();
                instance.last_translation = sample.translation;
                instance.last_rotation = sample.rotation;
                instance.last_scale = sample.scale;
                instance.last_tint = sample.tint;
                sample
            } else {
                let mut entity_mut = self.world.entity_mut(entity);
                let mut instance = ClipInstance::new(Arc::clone(&clip_name), Arc::clone(&clip_arc));
                let sample = instance.sample();
                instance.last_translation = sample.translation;
                instance.last_rotation = sample.rotation;
                instance.last_scale = sample.scale;
                instance.last_tint = sample.tint;
                if !entity_mut.contains::<TransformTrackPlayer>() {
                    entity_mut.insert(TransformTrackPlayer::default());
                }
                if instance.clip.tint.is_some() && !entity_mut.contains::<PropertyTrackPlayer>() {
                    entity_mut.insert(PropertyTrackPlayer::default());
                }
                entity_mut.insert(instance);
                sample
            }
        };
        self.apply_clip_sample_immediate(entity, sample);
        true
    }

    pub fn clear_transform_clip(&mut self, entity: Entity) -> bool {
        if self.world.get::<ClipInstance>(entity).is_some() {
            self.world.entity_mut(entity).remove::<ClipInstance>();
            true
        } else {
            false
        }
    }

    pub fn set_transform_clip_playing(&mut self, entity: Entity, playing: bool) -> bool {
        if let Some(mut instance) = self.world.get_mut::<ClipInstance>(entity) {
            instance.set_playing(playing);
            true
        } else {
            false
        }
    }

    pub fn set_transform_clip_speed(&mut self, entity: Entity, speed: f32) -> bool {
        if let Some(mut instance) = self.world.get_mut::<ClipInstance>(entity) {
            if speed.is_finite() {
                instance.set_speed(speed);
            }
            true
        } else {
            false
        }
    }

    pub fn set_transform_clip_group(&mut self, entity: Entity, group: Option<&str>) -> bool {
        if let Some(mut instance) = self.world.get_mut::<ClipInstance>(entity) {
            instance.set_group(group);
            true
        } else {
            false
        }
    }

    pub fn set_transform_clip_time(&mut self, entity: Entity, time: f32) -> bool {
        if let Some(mut instance) = self.world.get_mut::<ClipInstance>(entity) {
            instance.set_time(time);
            let sample = instance.sample();
            instance.last_translation = sample.translation;
            instance.last_rotation = sample.rotation;
            instance.last_scale = sample.scale;
            instance.last_tint = sample.tint;
            drop(instance);
            self.apply_clip_sample_immediate(entity, sample);
            true
        } else {
            false
        }
    }

    pub fn reset_transform_clip(&mut self, entity: Entity) -> bool {
        if let Some(mut instance) = self.world.get_mut::<ClipInstance>(entity) {
            instance.reset();
            let sample = instance.sample();
            instance.last_translation = sample.translation;
            instance.last_rotation = sample.rotation;
            instance.last_scale = sample.scale;
            instance.last_tint = sample.tint;
            drop(instance);
            self.apply_clip_sample_immediate(entity, sample);
            true
        } else {
            false
        }
    }

    pub fn set_transform_track_mask(&mut self, entity: Entity, mask: TransformTrackPlayer) -> bool {
        if self.world.get_entity(entity).is_err() {
            return false;
        }
        self.world.entity_mut(entity).insert(mask);
        true
    }

    pub fn set_property_track_mask(&mut self, entity: Entity, mask: PropertyTrackPlayer) -> bool {
        if self.world.get_entity(entity).is_err() {
            return false;
        }
        if mask.apply_tint {
            if self.world.get::<Tint>(entity).is_none() {
                self.world.entity_mut(entity).insert(Tint(Vec4::ONE));
            }
        }
        self.world.entity_mut(entity).insert(mask);
        true
    }

    fn apply_clip_sample_immediate(&mut self, entity: Entity, sample: ClipSample) {
        let transform_mask = self.world.get::<TransformTrackPlayer>(entity).copied().unwrap_or_default();
        if let Some(mut transform) = self.world.get_mut::<Transform>(entity) {
            if transform_mask.apply_translation {
                if let Some(value) = sample.translation {
                    transform.translation = value;
                }
            }
            if transform_mask.apply_rotation {
                if let Some(value) = sample.rotation {
                    transform.rotation = value;
                }
            }
            if transform_mask.apply_scale {
                if let Some(value) = sample.scale {
                    transform.scale = value;
                }
            }
        }
        let property_mask = self.world.get::<PropertyTrackPlayer>(entity).copied().unwrap_or_default();
        if let Some(mut tint) = self.world.get_mut::<Tint>(entity) {
            if property_mask.apply_tint {
                if let Some(value) = sample.tint {
                    tint.0 = value;
                }
            }
        }
    }

    pub fn set_sprite_atlas(&mut self, entity: Entity, assets: &AssetManager, atlas_key: &str) -> bool {
        if !assets.has_atlas(atlas_key) {
            return false;
        }
        {
            let Some(mut sprite) = self.world.get_mut::<Sprite>(entity) else {
                return false;
            };
            let mut desired_region = sprite.region.as_ref().to_string();
            if !assets.atlas_region_exists(atlas_key, &desired_region) {
                let region_names = assets.atlas_region_names(atlas_key);
                desired_region = match region_names.into_iter().next() {
                    Some(name) => name,
                    None => return false,
                };
            }
            let Some((region_name, region_info)) = assets.atlas_region_info(atlas_key, &desired_region)
            else {
                return false;
            };
            sprite.atlas_key = Arc::from(atlas_key.to_string());
            sprite.region = Arc::clone(region_name);
            sprite.region_id = region_info.id;
            sprite.uv = region_info.uv;
        }
        self.world.entity_mut(entity).remove::<SpriteAnimation>();
        true
    }
    pub fn set_sprite_region(&mut self, entity: Entity, assets: &AssetManager, region: &str) -> bool {
        if let Some(mut sprite) = self.world.get_mut::<Sprite>(entity) {
            let atlas_key = sprite.atlas_key.as_ref();
            let Some((name, info)) = assets.atlas_region_info(atlas_key, region) else {
                return false;
            };
            sprite.region = Arc::clone(name);
            sprite.region_id = info.id;
            sprite.uv = info.uv;
            drop(sprite);
            self.world.entity_mut(entity).remove::<SpriteAnimation>();
            true
        } else {
            false
        }
    }

    pub fn set_sprite_timeline(
        &mut self,
        entity: Entity,
        assets: &AssetManager,
        timeline: Option<&str>,
    ) -> bool {
        match timeline {
            Some(name) => {
                let previous_config = self
                    .world
                    .get::<SpriteAnimation>(entity)
                    .map(|anim| (anim.start_offset, anim.random_start, anim.group.clone()));
                let atlas = if let Some(sprite) = self.world.get::<Sprite>(entity) {
                    sprite.atlas_key.to_string()
                } else {
                    return false;
                };
                let definition = match assets.atlas_timeline(&atlas, name).cloned() {
                    Some(def) => def,
                    None => return false,
                };
                if definition.frames.is_empty() {
                    return false;
                }
                let frames = Arc::clone(&definition.frames);
                let durations = Arc::clone(&definition.durations);
                let loop_mode = definition.loop_mode;
                let component =
                    SpriteAnimation::new(Arc::clone(&definition.name), frames, durations, loop_mode);
                self.world.entity_mut(entity).insert(component);
                if let Some(mut animation) = self.world.get_mut::<SpriteAnimation>(entity) {
                    if let Some((offset, random, group)) = previous_config {
                        animation.start_offset = offset;
                        animation.random_start = random;
                        animation.group = group;
                    }
                }
                self.reset_sprite_animation(entity);
                self.reinitialize_sprite_animation_phase(entity);
                true
            }
            None => {
                self.world.entity_mut(entity).remove::<SpriteAnimation>();
                true
            }
        }
    }

    pub fn set_sprite_animation_playing(&mut self, entity: Entity, playing: bool) -> bool {
        if let Some(mut animation) = self.world.get_mut::<SpriteAnimation>(entity) {
            animation.playing = playing && !animation.frames.is_empty();
            true
        } else {
            false
        }
    }

    pub fn set_sprite_animation_speed(&mut self, entity: Entity, speed: f32) -> bool {
        if let Some(mut animation) = self.world.get_mut::<SpriteAnimation>(entity) {
            if speed.is_finite() {
                animation.set_speed(speed);
            }
            true
        } else {
            false
        }
    }

    pub fn set_sprite_animation_start_offset(&mut self, entity: Entity, offset: f32) -> bool {
        let Some(mut animation) = self.world.get_mut::<SpriteAnimation>(entity) else {
            return false;
        };
        animation.set_start_offset(offset);
        drop(animation);
        self.reinitialize_sprite_animation_phase(entity);
        true
    }

    pub fn set_sprite_animation_random_start(&mut self, entity: Entity, random: bool) -> bool {
        let Some(mut animation) = self.world.get_mut::<SpriteAnimation>(entity) else {
            return false;
        };
        animation.set_random_start(random);
        drop(animation);
        self.reinitialize_sprite_animation_phase(entity);
        true
    }

    pub fn set_sprite_animation_group(&mut self, entity: Entity, group: Option<&str>) -> bool {
        if let Some(mut animation) = self.world.get_mut::<SpriteAnimation>(entity) {
            animation.set_group(group.map(|value| value.to_string()));
            true
        } else {
            false
        }
    }

    pub fn set_animation_time_scale(&mut self, scale: f32) {
        self.world.resource_mut::<AnimationTime>().scale = scale.max(0.0);
    }

    pub fn set_animation_time_paused(&mut self, paused: bool) {
        self.world.resource_mut::<AnimationTime>().paused = paused;
    }

    pub fn set_animation_time_fixed_step(&mut self, step: Option<f32>) {
        self.world.resource_mut::<AnimationTime>().set_fixed_step(step);
    }

    pub fn set_animation_group_scale(&mut self, group: &str, scale: f32) {
        {
            let mut animation_time = self.world.resource_mut::<AnimationTime>();
            animation_time.set_group_scale(group, scale);
        }
        let mut query = self.world.query::<&mut SpriteAnimation>();
        for mut animation in query.iter_mut(&mut self.world) {
            if animation.group.as_deref() == Some(group) {
                animation.mark_playback_rate_dirty();
            }
        }
    }

    fn reinitialize_sprite_animation_phase(&mut self, entity: Entity) {
        let snapshot = if let Some(mut animation) = self.world.get_mut::<SpriteAnimation>(entity) {
            initialize_animation_phase(&mut animation, entity);
            Self::current_frame_snapshot(&animation)
        } else {
            None
        };
        self.apply_sprite_snapshot(entity, snapshot);
    }

    pub fn set_sprite_animation_looped(&mut self, entity: Entity, looped: bool) -> bool {
        if let Some(mut animation) = self.world.get_mut::<SpriteAnimation>(entity) {
            if looped {
                if !animation.mode.looped() {
                    animation.set_mode(SpriteAnimationLoopMode::Loop);
                } else {
                    animation.looped = true;
                }
            } else if animation.mode.looped() {
                animation.set_mode(SpriteAnimationLoopMode::OnceStop);
            } else {
                animation.looped = false;
            }
            true
        } else {
            false
        }
    }

    pub fn set_sprite_animation_loop_mode(&mut self, entity: Entity, mode: SpriteAnimationLoopMode) -> bool {
        if let Some(mut animation) = self.world.get_mut::<SpriteAnimation>(entity) {
            animation.set_mode(mode);
            true
        } else {
            false
        }
    }

    pub fn seek_sprite_animation_frame(&mut self, entity: Entity, frame: usize) -> bool {
        let Some(mut animation) = self.world.get_mut::<SpriteAnimation>(entity) else {
            return false;
        };
        if animation.frames.is_empty() {
            return false;
        }
        let target = frame.min(animation.frames.len() - 1);
        let snapshot = if animation.frame_index != target || animation.elapsed_in_frame != 0.0 {
            animation.frame_index = target;
            animation.elapsed_in_frame = 0.0;
            animation.refresh_current_duration();
            Self::current_frame_snapshot(&animation)
        } else {
            None
        };
        drop(animation);
        self.apply_sprite_snapshot(entity, snapshot);
        true
    }

    pub fn refresh_sprite_animations_for_atlas(&mut self, atlas_key: &str, assets: &AssetManager) -> usize {
        let mut updated = 0usize;
        let mut query = self.world.query::<(Entity, &mut Sprite, &mut SpriteAnimation)>();
        for (entity, mut sprite, mut animation) in query.iter_mut(&mut self.world) {
            if sprite.atlas_key.as_ref() != atlas_key {
                continue;
            }
            let timeline_name = animation.timeline.clone();
            let Some(definition) = assets.atlas_timeline(atlas_key, timeline_name.as_ref()) else {
                animation.frames = Arc::from(Vec::<SpriteAnimationFrame>::new());
                animation.frame_durations = Arc::from(Vec::<f32>::new());
                animation.has_events = false;
                animation.fast_loop = false;
                animation.frame_index = 0;
                animation.elapsed_in_frame = 0.0;
                animation.refresh_current_duration();
                animation.playing = false;
                animation.refresh_current_duration();
                updated += 1;
                eprintln!(
                    "[assets] Atlas '{atlas_key}' no longer defines timeline '{}' (entity {:?})",
                    timeline_name, entity
                );
                continue;
            };
            let prev_frames: Vec<SpriteAnimationFrame> = animation.frames.iter().cloned().collect();
            let prev_index = animation.frame_index;
            let prev_frame = prev_frames.get(prev_index).cloned();
            let prev_elapsed = animation.elapsed_in_frame;
            let prev_playing = animation.playing;
            let prev_forward = animation.forward;
            let prev_speed = animation.speed;

            animation.frames = Arc::clone(&definition.frames);
            animation.frame_durations = Arc::clone(&definition.durations);
            animation.timeline = Arc::clone(&definition.name);
            animation.has_events = animation.frames.iter().any(|frame| !frame.events.is_empty());
            animation.fast_loop =
                !animation.has_events && matches!(animation.mode, SpriteAnimationLoopMode::Loop);

            if animation.frames.is_empty() {
                animation.frame_index = 0;
                animation.elapsed_in_frame = 0.0;
                animation.playing = false;
                animation.refresh_current_duration();
                updated += 1;
                continue;
            }
            animation.set_mode(definition.loop_mode);

            let new_len = animation.frames.len();
            let mut target_index = prev_index.min(new_len - 1);
            let mut matched = false;
            if let Some(prev_frame) = prev_frame.as_ref() {
                let target_name = prev_frame.name.as_ref();
                let occurrence = prev_frames[..=prev_index]
                    .iter()
                    .filter(|frame| frame.name.as_ref() == target_name)
                    .count();
                let mut seen = 0usize;
                if let Some(found) = animation.frames.iter().position(|frame| {
                    if frame.name.as_ref() == target_name {
                        seen += 1;
                        if seen == occurrence {
                            return true;
                        }
                    }
                    false
                }) {
                    target_index = found;
                    matched = true;
                }
            }
            if !matched {
                if let Some(prev_frame) = prev_frame.as_ref() {
                    let target_name = prev_frame.name.as_ref();
                    if let Some(found) =
                        animation.frames.iter().position(|frame| frame.name.as_ref() == target_name)
                    {
                        target_index = found;
                        matched = true;
                    }
                }
            }
            if !matched {
                if let Some(prev_frame) = prev_frame.as_ref() {
                    let target_region = prev_frame.region.as_ref();
                    let occurrence = prev_frames[..=prev_index]
                        .iter()
                        .filter(|frame| frame.region.as_ref() == target_region)
                        .count();
                    let mut seen = 0usize;
                    if let Some(found) = animation.frames.iter().position(|frame| {
                        if frame.region.as_ref() == target_region {
                            seen += 1;
                            if seen == occurrence {
                                return true;
                            }
                        }
                        false
                    }) {
                        target_index = found;
                        matched = true;
                    }
                }
            }
            if !matched {
                if let Some(prev_frame) = prev_frame.as_ref() {
                    let target_region = prev_frame.region.as_ref();
                    if let Some(found) =
                        animation.frames.iter().position(|frame| frame.region.as_ref() == target_region)
                    {
                        target_index = found;
                        matched = true;
                    }
                }
            }
            if !matched && prev_index >= new_len {
                target_index = new_len - 1;
            }
            animation.frame_index = target_index;
            animation.refresh_current_duration();
            let new_duration = animation.current_duration.max(std::f32::EPSILON);
            let prev_duration = prev_frames
                .get(prev_index)
                .map(|frame| frame.duration)
                .unwrap_or(std::f32::EPSILON)
                .max(std::f32::EPSILON);
            let progress = (prev_elapsed / prev_duration).clamp(0.0, 1.0);
            animation.elapsed_in_frame = (progress * new_duration).min(new_duration);
            animation.playing = prev_playing && !animation.frames.is_empty();
            animation.forward = prev_forward;
            animation.set_speed(prev_speed);

            if let Some(frame) = animation.current_frame() {
                sprite.apply_frame(frame);
            }

            updated += 1;
        }
        updated
    }

    pub fn reset_sprite_animation(&mut self, entity: Entity) -> bool {
        if let Some(mut animation) = self.world.get_mut::<SpriteAnimation>(entity) {
            if animation.frames.is_empty() {
                return false;
            }
            animation.frame_index = 0;
            animation.elapsed_in_frame = 0.0;
            animation.playing = true;
            animation.forward = true;
            animation.refresh_current_duration();
            let snapshot = Self::current_frame_snapshot(&animation);
            drop(animation);
            self.apply_sprite_snapshot(entity, snapshot);
            true
        } else {
            false
        }
    }
    pub fn collect_sprite_instances(&mut self, assets: &AssetManager) -> Result<Vec<SpriteInstance>> {
        let mut out = Vec::new();
        let mut q = self.world.query::<(&WorldTransform, &mut Sprite, Option<&Tint>)>();
        for (wt, mut sprite, tint) in q.iter_mut(&mut self.world) {
            let atlas_key = Arc::clone(&sprite.atlas_key);
            let atlas_key_str = atlas_key.as_ref();
            let uv_rect = if sprite.is_initialized() {
                sprite.uv
            } else {
                if let Some((region, info)) = assets.atlas_region_info(atlas_key_str, sprite.region.as_ref())
                {
                    sprite.region = region.clone();
                    sprite.region_id = info.id;
                    sprite.uv = info.uv;
                    info.uv
                } else {
                    sprite.uv
                }
            };
            let color = tint.map(|t| t.0.to_array()).unwrap_or([1.0, 1.0, 1.0, 1.0]);
            out.push(SpriteInstance {
                atlas: atlas_key.to_string(),
                data: InstanceData { model: wt.0.to_cols_array_2d(), uv_rect, tint: color },
            });
        }
        Ok(out)
    }

    pub fn collect_mesh_instances(&mut self) -> Vec<MeshInstance> {
        let mut instances = Vec::new();
        let mut query = self.world.query::<(&WorldTransform3D, &MeshRef, Option<&MeshSurface>)>();
        for (wt, mesh, surface) in query.iter(&self.world) {
            let lighting = surface.map(|s| MeshLightingInfo::from(&s.lighting)).unwrap_or_default();
            let material = surface.and_then(|s| s.material.clone());
            instances.push(MeshInstance { key: mesh.key.clone(), model: wt.0, material, lighting });
        }
        instances
    }

    pub fn set_mesh_translation(&mut self, entity: Entity, translation: Vec3) -> bool {
        if let Some(mut transform) = self.world.get_mut::<Transform3D>(entity) {
            transform.translation = translation;
            let updated = *transform;
            drop(transform);
            if let Some(mut transform2d) = self.world.get_mut::<Transform>(entity) {
                transform2d.translation = Vec2::new(translation.x, translation.y);
            }
            self.update_world_transform3d(entity, updated);
            true
        } else {
            false
        }
    }

    pub fn set_mesh_scale(&mut self, entity: Entity, scale: Vec3) -> bool {
        if let Some(mut transform) = self.world.get_mut::<Transform3D>(entity) {
            transform.scale = scale;
            let updated = *transform;
            drop(transform);
            if let Some(mut transform2d) = self.world.get_mut::<Transform>(entity) {
                transform2d.scale = Vec2::new(scale.x, scale.y);
            }
            self.update_world_transform3d(entity, updated);
            true
        } else {
            false
        }
    }

    pub fn set_mesh_rotation_euler(&mut self, entity: Entity, euler: Vec3) -> bool {
        if let Some(mut transform) = self.world.get_mut::<Transform3D>(entity) {
            transform.rotation = Quat::from_euler(EulerRot::XYZ, euler.x, euler.y, euler.z);
            let updated = *transform;
            drop(transform);
            self.update_world_transform3d(entity, updated);
            true
        } else {
            false
        }
    }

    pub fn set_mesh_material(&mut self, entity: Entity, material: Option<String>) -> bool {
        if let Some(mut surface) = self.world.get_mut::<MeshSurface>(entity) {
            surface.material = material;
            true
        } else {
            false
        }
    }

    pub fn set_mesh_shadow_flags(&mut self, entity: Entity, cast: bool, receive: bool) -> bool {
        if let Some(mut surface) = self.world.get_mut::<MeshSurface>(entity) {
            surface.lighting.cast_shadows = cast;
            surface.lighting.receive_shadows = receive;
            true
        } else {
            false
        }
    }

    pub fn set_mesh_material_params(
        &mut self,
        entity: Entity,
        base_color: Vec3,
        metallic: f32,
        roughness: f32,
        emissive: Option<Vec3>,
    ) -> bool {
        if let Some(mut surface) = self.world.get_mut::<MeshSurface>(entity) {
            surface.lighting.base_color = base_color.clamp(Vec3::ZERO, Vec3::splat(1.0));
            surface.lighting.metallic = metallic.clamp(0.0, 1.0);
            surface.lighting.roughness = roughness.clamp(0.04, 1.0);
            surface.lighting.emissive = emissive;
            true
        } else {
            false
        }
    }

    fn update_world_transform3d(&mut self, entity: Entity, transform: Transform3D) {
        if let Some(mut world) = self.world.get_mut::<WorldTransform3D>(entity) {
            let mat = Mat4::from_scale_rotation_translation(
                transform.scale,
                transform.rotation,
                transform.translation,
            );
            world.0 = mat;
            if let Some(mut world2d) = self.world.get_mut::<WorldTransform>(entity) {
                world2d.0 = mat;
            }
        }
    }
    pub fn entity_count(&self) -> usize {
        let boundary = self.world.resource::<RapierState>().boundary_entity();
        self.world.iter_entities().filter(|entity_ref| entity_ref.id() != boundary).count()
    }
    pub fn set_spatial_cell(&mut self, cell: f32) {
        let mut grid = self.world.resource_mut::<SpatialHash>();
        grid.cell = cell;
    }

    pub fn set_spatial_quadtree_enabled(&mut self, enabled: bool) {
        let mut config = self.world.resource_mut::<SpatialIndexConfig>();
        config.fallback_enabled = enabled;
    }

    pub fn set_spatial_density_threshold(&mut self, threshold: f32) {
        let mut config = self.world.resource_mut::<SpatialIndexConfig>();
        config.density_threshold = threshold.max(1.0);
    }

    pub fn spatial_metrics(&self) -> SpatialMetrics {
        *self.world.resource::<SpatialMetrics>()
    }

    pub fn profiler_begin_frame(&mut self) {
        self.world.resource_mut::<SystemProfiler>().begin_frame();
    }

    pub fn system_timings(&self) -> Vec<SystemTimingSummary> {
        self.world.resource::<SystemProfiler>().summaries()
    }

    pub fn pick_entity_3d(
        &mut self,
        origin: Vec3,
        direction: Vec3,
        registry: &MeshRegistry,
    ) -> Option<Entity> {
        let dir = direction.normalize_or_zero();
        if dir.length_squared() <= f32::EPSILON {
            return None;
        }
        let mut query = self.world.query::<(Entity, Option<&Transform3D>, &MeshRef)>();
        let mut closest: Option<(Entity, f32)> = None;
        for (entity, transform3d, mesh_ref) in query.iter(&self.world) {
            let Some(bounds) = registry.mesh_bounds(&mesh_ref.key) else {
                continue;
            };
            let mut hit_record: Option<f32> = None;
            if let Some(transform) = transform3d {
                if let Some(distance) = ray_hit_obb(origin, dir, transform, bounds) {
                    hit_record = Some(distance);
                }
            }
            if hit_record.is_none() {
                let (center, radius) = if let Some(transform) = transform3d {
                    let max_scale = transform
                        .scale
                        .x
                        .abs()
                        .max(transform.scale.y.abs())
                        .max(transform.scale.z.abs())
                        .max(0.0001);
                    (transform.translation, bounds.radius * max_scale)
                } else {
                    let center2 = self
                        .world
                        .get::<Transform>(entity)
                        .map(|t| Vec3::new(t.translation.x, t.translation.y, 0.0))
                        .unwrap_or(Vec3::ZERO);
                    (center2, bounds.radius)
                };
                if radius > 0.0 {
                    if let Some(distance) = ray_sphere_intersection(origin, dir, center, radius) {
                        hit_record = Some(distance);
                    }
                }
            }
            if let Some(distance) = hit_record {
                match closest {
                    Some((_, best)) if distance >= best => {}
                    _ => closest = Some((entity, distance)),
                }
            }
        }
        closest.map(|(entity, _)| entity)
    }

    pub fn pick_entity(&mut self, world_pos: Vec2) -> Option<Entity> {
        let mut query = self.world.query::<(Entity, &WorldTransform, Option<&Aabb>)>();
        query.iter(&self.world).find_map(|(entity, wt, aabb)| {
            let center = Vec2::new(wt.0.w_axis.x, wt.0.w_axis.y);
            let half = aabb.map_or(Vec2::splat(0.25), |a| a.half);
            if (world_pos.x - center.x).abs() <= half.x && (world_pos.y - center.y).abs() <= half.y {
                Some(entity)
            } else {
                None
            }
        })
    }
    pub fn entity_bounds(&self, entity: Entity) -> Option<(Vec2, Vec2)> {
        let wt = self.world.get::<WorldTransform>(entity)?;
        let center = Vec2::new(wt.0.w_axis.x, wt.0.w_axis.y);
        let half = self.world.get::<Aabb>(entity).map(|a| a.half).unwrap_or(Vec2::splat(0.25));
        Some((center - half, center + half))
    }

    pub fn collider_rects(&mut self) -> Vec<(Vec2, Vec2)> {
        let mut rects = Vec::new();
        let mut query = self.world.query::<(&WorldTransform, &Aabb)>();
        for (wt, aabb) in query.iter(&self.world) {
            let center = Vec2::new(wt.0.w_axis.x, wt.0.w_axis.y);
            rects.push((center - aabb.half, center + aabb.half));
        }
        rects
    }

    pub fn spatial_hash_rects(&self) -> Vec<(Vec2, Vec2)> {
        let grid = self.world.resource::<SpatialHash>();
        let cell = grid.cell;
        let mut rects = Vec::with_capacity(grid.grid.len());
        for ((ix, iy), _) in grid.grid.iter() {
            let min = Vec2::new(*ix as f32 * cell, *iy as f32 * cell);
            let max = min + Vec2::splat(cell);
            rects.push((min, max));
        }
        rects
    }

    pub fn find_entity_by_scene_id(&mut self, scene_id: &str) -> Option<Entity> {
        let mut query = self.world.query::<(Entity, &SceneEntityTag)>();
        for (entity, tag) in query.iter(&self.world) {
            if tag.id.as_str() == scene_id {
                return Some(entity);
            }
        }
        None
    }

    pub fn entity_info(&self, entity: Entity) -> Option<EntityInfo> {
        let transform = self.world.get::<Transform>(entity)?;
        let world_transform = self.world.get::<WorldTransform>(entity)?;
        let translation = Vec2::new(world_transform.0.w_axis.x, world_transform.0.w_axis.y);
        let scene_id = self.world.get::<SceneEntityTag>(entity)?.id.clone();
        let velocity = self.world.get::<Velocity>(entity).map(|v| v.0);
        let transform_tracks = self.world.get::<TransformTrackPlayer>(entity).copied();
        let property_tracks = self.world.get::<PropertyTrackPlayer>(entity).copied();
        let transform_clip = self.world.get::<ClipInstance>(entity).map(|instance| {
            let clip = Arc::clone(&instance.clip);
            let sample = instance.sample();
            TransformClipInfo {
                clip_key: instance.clip_key.as_ref().to_string(),
                playing: instance.playing,
                looped: instance.looped,
                speed: instance.speed,
                time: instance.time,
                duration: instance.duration(),
                group: instance.group.clone(),
                has_translation: clip.translation.is_some(),
                has_rotation: clip.rotation.is_some(),
                has_scale: clip.scale.is_some(),
                has_tint: clip.tint.is_some(),
                sample_translation: sample.translation,
                sample_rotation: sample.rotation,
                sample_scale: sample.scale,
                sample_tint: sample.tint,
            }
        });
        let sprite = if let Some(sprite) = self.world.get::<Sprite>(entity) {
            let atlas = sprite.atlas_key.to_string();
            let region = sprite.region.to_string();
            let animation = self.world.get::<SpriteAnimation>(entity).map(|anim| {
                let frame = anim.frames.get(anim.frame_index);
                let frame_region = frame.map(|frame| frame.region.as_ref().to_string());
                let frame_region_id = frame.map(|frame| frame.region_id);
                let frame_uv = frame.map(|frame| frame.uv);
                let frame_duration = frame.map(|frame| frame.duration).unwrap_or(0.0);
                let frame_events = frame
                    .map(|frame| frame.events.iter().map(|e| e.as_ref().to_string()).collect())
                    .unwrap_or_default();
                SpriteAnimationInfo {
                    timeline: anim.timeline.as_ref().to_string(),
                    playing: anim.playing,
                    looped: anim.looped,
                    loop_mode: anim.mode.as_str().to_string(),
                    speed: anim.speed,
                    frame_index: anim.frame_index,
                    frame_count: anim.frame_count(),
                    frame_elapsed: anim.elapsed_in_frame,
                    frame_duration,
                    frame_region,
                    frame_region_id,
                    frame_uv,
                    frame_events,
                    start_offset: anim.start_offset,
                    random_start: anim.random_start,
                    group: anim.group.clone(),
                }
            });
            Some(SpriteInfo { atlas, region, animation })
        } else {
            None
        };
        let mesh_surface = self.world.get::<MeshSurface>(entity);
        let mesh = self.world.get::<MeshRef>(entity).map(|mesh_ref| {
            let material = mesh_surface.and_then(|surface| surface.material.clone());
            let lighting =
                mesh_surface.map(|surface| MeshLightingInfo::from(&surface.lighting)).unwrap_or_default();
            MeshInfo { key: mesh_ref.key.clone(), material, lighting }
        });
        let mesh_transform = self.world.get::<Transform3D>(entity).map(|transform| Transform3DInfo {
            translation: transform.translation,
            rotation: transform.rotation,
            scale: transform.scale,
        });
        let tint = self.world.get::<Tint>(entity).map(|t| t.0);
        Some(EntityInfo {
            scene_id,
            translation,
            rotation: transform.rotation,
            scale: transform.scale,
            velocity,
            transform_clip,
            transform_tracks,
            property_tracks,
            sprite,
            mesh,
            mesh_transform,
            tint,
        })
    }
    pub fn entity_exists(&self, entity: Entity) -> bool {
        self.world.get_entity(entity).is_ok()
    }
    pub fn despawn_entity(&mut self, entity: Entity) -> bool {
        if let Some(parent) = self.world.get::<Parent>(entity).copied() {
            if let Some(mut siblings) = self.world.get_mut::<Children>(parent.0) {
                siblings.0.retain(|&child| child != entity);
            }
        }
        let child_ids = self.world.get::<Children>(entity).map(|c| c.0.clone()).unwrap_or_default();
        let mut removed = false;
        for child in child_ids {
            removed |= self.despawn_entity(child);
        }
        if let Some(handle) = self.world.get::<RapierBody>(entity).map(|b| b.handle) {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.remove_body(handle);
        }
        let entity_removed = self.world.despawn(entity);
        if entity_removed {
            removed = true;
            self.emit(GameEvent::EntityDespawned { entity });
        }
        removed
    }
    pub fn set_root_spin(&mut self, speed: f32) {
        let mut query = self.world.query::<&mut Spin>();
        for mut spin in query.iter_mut(&mut self.world) {
            spin.speed = speed;
            break;
        }
    }
}

impl EcsWorld {
    pub fn save_scene_to_path(&mut self, path: impl AsRef<Path>, assets: &AssetManager) -> Result<()> {
        let scene = self.export_scene_with_mesh_source(assets, |_| None);
        let missing_mesh: Vec<String> =
            scene.dependencies.mesh_dependencies().map(|dep| dep.key().to_string()).collect();
        if !missing_mesh.is_empty() {
            return Err(anyhow!(
                "Scene references meshes ({}) but no mesh source exporter was provided. Use \
                 save_scene_to_path_with_mesh_source to record mesh paths.",
                missing_mesh.join(", ")
            ));
        }
        scene.save_to_path(path)
    }

    pub fn save_scene_to_path_with_mesh_source<F>(
        &mut self,
        path: impl AsRef<Path>,
        assets: &AssetManager,
        mesh_source: F,
    ) -> Result<()>
    where
        F: Fn(&str) -> Option<String>,
    {
        self.save_scene_to_path_with_sources(path, assets, mesh_source, |_| None)
    }

    pub fn save_scene_to_path_with_sources<F, G>(
        &mut self,
        path: impl AsRef<Path>,
        assets: &AssetManager,
        mesh_source: F,
        material_source: G,
    ) -> Result<()>
    where
        F: Fn(&str) -> Option<String>,
        G: Fn(&str) -> Option<String>,
    {
        let scene = self.export_scene_with_sources(assets, mesh_source, material_source);
        scene.save_to_path(path)
    }

    pub fn load_scene_from_path(
        &mut self,
        path: impl AsRef<Path>,
        assets: &mut AssetManager,
    ) -> Result<Scene> {
        self.load_scene_from_path_with_mesh(path, assets, |_key, _path| {
            Err(anyhow!(
                "Scene references meshes but no mesh resolver was provided. Use load_scene_from_path_with_mesh."
            ))
        })
    }

    pub fn load_scene_from_path_with_mesh<F>(
        &mut self,
        path: impl AsRef<Path>,
        assets: &mut AssetManager,
        mesh_loader: F,
    ) -> Result<Scene>
    where
        F: FnMut(&str, Option<&str>) -> Result<()>,
    {
        let scene = Scene::load_from_path(path)?;
        self.ensure_scene_dependencies_with_mesh(&scene, assets, mesh_loader)?;
        self.load_scene_internal(&scene, assets)?;
        Ok(scene)
    }

    fn ensure_scene_dependencies_with_mesh<F>(
        &self,
        scene: &Scene,
        assets: &mut AssetManager,
        mut mesh_loader: F,
    ) -> Result<()>
    where
        F: FnMut(&str, Option<&str>) -> Result<()>,
    {
        let mut missing = Vec::new();
        for dep in scene.dependencies.atlas_dependencies() {
            if assets.has_atlas(dep.key()) {
                continue;
            }
            if let Some(path) = dep.path() {
                if let Err(err) = assets.load_atlas(dep.key(), path) {
                    missing.push(format!("{} ({}): {err}", dep.key(), path));
                }
            } else {
                missing.push(format!("{} (no path provided)", dep.key()));
            }
        }
        if !missing.is_empty() {
            return Err(anyhow!("Scene requires atlases that could not be loaded: {}", missing.join(", ")));
        }

        let mut mesh_missing = Vec::new();
        for dep in scene.dependencies.mesh_dependencies() {
            if let Err(err) = mesh_loader(dep.key(), dep.path()) {
                let source = dep.path().unwrap_or("no path provided");
                mesh_missing.push(format!("{} ({source}) : {err}", dep.key()));
            }
        }
        if !mesh_missing.is_empty() {
            return Err(anyhow!(
                "Scene requires meshes that could not be prepared: {}",
                mesh_missing.join(", ")
            ));
        }
        Ok(())
    }

    pub fn load_scene(&mut self, scene: &Scene, assets: &AssetManager) -> Result<()> {
        self.load_scene_with_mesh(scene, assets, |_key, _path| {
            Err(anyhow!(
                "Scene references meshes but no mesh resolver was provided. Use load_scene_with_mesh."
            ))
        })
    }

    pub fn load_scene_with_mesh<F>(
        &mut self,
        scene: &Scene,
        assets: &AssetManager,
        mut mesh_loader: F,
    ) -> Result<()>
    where
        F: FnMut(&str, Option<&str>) -> Result<()>,
    {
        for dep in scene.dependencies.atlas_dependencies() {
            if !assets.has_atlas(dep.key()) {
                return Err(anyhow!(
                    "Scene requires atlas '{}' which is not loaded. Call AssetManager::load_atlas before loading the scene.",
                    dep.key()
                ));
            }
        }
        let mut mesh_missing = Vec::new();
        for dep in scene.dependencies.mesh_dependencies() {
            if let Err(err) = mesh_loader(dep.key(), dep.path()) {
                let source = dep.path().unwrap_or("no path provided");
                mesh_missing.push(format!("{} ({source}) : {err}", dep.key()));
            }
        }
        if !mesh_missing.is_empty() {
            return Err(anyhow!("Scene requires meshes that are unavailable: {}", mesh_missing.join(", ")));
        }
        self.load_scene_internal(scene, assets)
    }

    fn load_scene_internal(&mut self, scene: &Scene, assets: &AssetManager) -> Result<()> {
        self.clear_scene_entities();
        let mut entity_map = Vec::with_capacity(scene.entities.len());
        let mut id_map: HashMap<SceneEntityId, Entity> = HashMap::with_capacity(scene.entities.len());
        for entity_data in &scene.entities {
            let entity = self.spawn_scene_entity(entity_data, assets)?;
            entity_map.push(entity);
            id_map.insert(entity_data.id.clone(), entity);
        }
        for (index, entity_data) in scene.entities.iter().enumerate() {
            let child_entity = entity_map[index];
            if let Some(parent_id) = entity_data.parent_id.as_ref() {
                if let Some(&parent_entity) = id_map.get(parent_id) {
                    self.attach_child_to_parent(child_entity, parent_entity);
                    continue;
                }
            }
            if let Some(parent_index) = entity_data.parent {
                let parent_entity = *entity_map
                    .get(parent_index)
                    .ok_or_else(|| anyhow!("Scene entity parent index {parent_index} out of bounds"))?;
                self.attach_child_to_parent(child_entity, parent_entity);
            }
        }
        Ok(())
    }

    fn attach_child_to_parent(&mut self, child_entity: Entity, parent_entity: Entity) {
        self.world.entity_mut(child_entity).insert(Parent(parent_entity));
        if let Some(mut children) = self.world.get_mut::<Children>(parent_entity) {
            if !children.0.contains(&child_entity) {
                children.0.push(child_entity);
            }
        } else {
            self.world.entity_mut(parent_entity).insert(Children(vec![child_entity]));
        }
    }

    fn ensure_scene_entity_tag(&mut self, entity: Entity) -> SceneEntityId {
        if let Some(tag) = self.world.get::<SceneEntityTag>(entity).cloned() {
            return tag.id;
        }
        let id = SceneEntityId::new();
        self.world.entity_mut(entity).insert(SceneEntityTag::new(id.clone()));
        id
    }

    pub fn export_scene(&mut self, assets: &AssetManager) -> Scene {
        self.export_scene_with_sources(assets, |_| None, |_| None)
    }

    pub fn export_scene_with_mesh_source<F>(&mut self, assets: &AssetManager, mesh_source: F) -> Scene
    where
        F: Fn(&str) -> Option<String>,
    {
        self.export_scene_with_sources(assets, mesh_source, |_| None)
    }

    pub fn export_scene_with_sources<F, G>(
        &mut self,
        assets: &AssetManager,
        mesh_source: F,
        material_source: G,
    ) -> Scene
    where
        F: Fn(&str) -> Option<String>,
        G: Fn(&str) -> Option<String>,
    {
        let mut scene = Scene::default();
        let mut query = self.world.query::<(Entity, Option<&Parent>, Option<&Transform>)>();
        let mut roots = Vec::new();
        for (entity, parent, transform) in query.iter(&self.world) {
            if parent.is_none() && transform.is_some() {
                roots.push(entity);
            }
        }
        for root in roots {
            self.collect_scene_entity(root, None, None, &mut scene.entities);
        }
        scene.dependencies =
            SceneDependencies::from_entities(&scene.entities, assets, mesh_source, material_source);
        scene
    }

    pub fn export_prefab(&mut self, root: Entity, assets: &AssetManager) -> Option<Scene> {
        if !self.entity_exists(root) {
            return None;
        }
        let mut entities = Vec::new();
        self.collect_scene_entity(root, None, None, &mut entities);
        if entities.is_empty() {
            return None;
        }
        let dependencies = SceneDependencies::from_entities(&entities, assets, |_| None, |_| None);
        let mut scene = Scene::default();
        scene.entities = entities;
        scene.dependencies = dependencies;
        Some(scene)
    }

    pub fn instantiate_prefab(&mut self, scene: &Scene, assets: &AssetManager) -> Result<Vec<Entity>> {
        self.instantiate_scene_entities(scene, assets)
    }

    pub fn instantiate_prefab_with_mesh<F>(
        &mut self,
        scene: &Scene,
        assets: &mut AssetManager,
        mut mesh_loader: F,
    ) -> Result<Vec<Entity>>
    where
        F: FnMut(&str, Option<&str>) -> Result<()>,
    {
        self.ensure_scene_dependencies_with_mesh(scene, assets, &mut mesh_loader)?;
        self.instantiate_scene_entities(scene, assets)
    }

    pub fn first_emitter(&mut self) -> Option<Entity> {
        let mut query = self.world.query::<(Entity, &ParticleEmitter)>();
        query.iter(&self.world).map(|(entity, _)| entity).next()
    }

    pub fn emitter_snapshot(&self, entity: Entity) -> Option<EmitterSnapshot> {
        let emitter = self.world.get::<ParticleEmitter>(entity)?;
        Some(EmitterSnapshot {
            rate: emitter.rate,
            spread: emitter.spread,
            speed: emitter.speed,
            lifetime: emitter.lifetime,
            start_color: emitter.start_color,
            end_color: emitter.end_color,
            start_size: emitter.start_size,
            end_size: emitter.end_size,
        })
    }

    fn instantiate_scene_entities(&mut self, scene: &Scene, assets: &AssetManager) -> Result<Vec<Entity>> {
        if scene.entities.is_empty() {
            return Ok(Vec::new());
        }
        let mut entity_map = Vec::with_capacity(scene.entities.len());
        let mut id_map: HashMap<SceneEntityId, Entity> = HashMap::with_capacity(scene.entities.len());
        for entity_data in &scene.entities {
            let entity = self.spawn_scene_entity(entity_data, assets)?;
            id_map.insert(entity_data.id.clone(), entity);
            entity_map.push(entity);
        }
        for (index, entity_data) in scene.entities.iter().enumerate() {
            let child_entity = entity_map[index];
            if let Some(parent_id) = entity_data.parent_id.as_ref() {
                if let Some(&parent_entity) = id_map.get(parent_id) {
                    self.attach_child_to_parent(child_entity, parent_entity);
                    continue;
                }
            }
            if let Some(parent_index) = entity_data.parent {
                if let Some(&parent_entity) = entity_map.get(parent_index) {
                    self.attach_child_to_parent(child_entity, parent_entity);
                }
            }
        }
        Ok(entity_map)
    }

    fn spawn_scene_entity(&mut self, data: &SceneEntity, assets: &AssetManager) -> Result<Entity> {
        let translation: Vec2 = data.transform.translation.clone().into();
        let scale: Vec2 = data.transform.scale.clone().into();
        let rotation = data.transform.rotation;
        let velocity_vec: Vec2 = data.velocity.as_ref().map(|v| Vec2::from(v.clone())).unwrap_or(Vec2::ZERO);
        let collider_half = data.collider.as_ref().map(|c| Vec2::from(c.half_extents.clone()));

        let mut body_handle = None;
        let mut collider_handle = None;
        if let Some(half) = collider_half.as_ref() {
            let mass_value = data.mass.unwrap_or(1.0);
            let mut rapier = self.world.resource_mut::<RapierState>();
            let (body, collider) = rapier.spawn_dynamic_body(translation, *half, mass_value, velocity_vec);
            body_handle = Some(body);
            collider_handle = Some(collider);
        }

        let mut entity =
            self.world.spawn((Transform { translation, rotation, scale }, WorldTransform::default()));
        entity.insert(SceneEntityTag::new(data.id.clone()));

        if let Some(transform3d) = data.transform3d.as_ref() {
            let (translation3, rotation3, scale3) = transform3d.components();
            let transform3d = Transform3D { translation: translation3, rotation: rotation3, scale: scale3 };
            entity.insert(transform3d);
            entity.insert(WorldTransform3D(Mat4::from_scale_rotation_translation(
                scale3,
                rotation3,
                translation3,
            )));
        } else if data.mesh.is_some() {
            let translation3 = Vec3::new(translation.x, translation.y, 0.0);
            let transform3d =
                Transform3D { translation: translation3, rotation: Quat::IDENTITY, scale: Vec3::ONE };
            entity.insert(transform3d);
            entity.insert(WorldTransform3D(Mat4::from_scale_rotation_translation(
                transform3d.scale,
                transform3d.rotation,
                transform3d.translation,
            )));
        }

        if let Some(spin) = data.spin {
            entity.insert(Spin { speed: spin });
        }
        if let Some(tint) = data.tint.clone() {
            entity.insert(Tint(tint.into()));
        }
        if let Some(velocity) = data.velocity.as_ref() {
            entity.insert(Velocity(velocity.clone().into()));
        }
        if let Some(mass) = data.mass {
            entity.insert(Mass(mass));
        }
        if let Some(half) = collider_half.as_ref() {
            entity.insert(Aabb { half: *half });
            entity.insert(Force::default());
        }
        if let Some(emitter) = data.particle_emitter.clone() {
            entity.insert(ParticleEmitter {
                rate: emitter.rate,
                spread: emitter.spread,
                speed: emitter.speed,
                lifetime: emitter.lifetime,
                accumulator: 0.0,
                start_color: emitter.start_color.into(),
                end_color: emitter.end_color.into(),
                start_size: emitter.start_size,
                end_size: emitter.end_size,
            });
        }
        if let Some(orbit) = data.orbit.clone() {
            entity
                .insert(OrbitController { center: orbit.center.into(), angular_speed: orbit.angular_speed });
        }

        if let Some(sprite) = data.sprite.as_ref() {
            let Some((region_name, info)) = assets.atlas_region_info(&sprite.atlas, &sprite.region) else {
                return Err(anyhow!(
                    "Scene references unknown atlas region '{}:{}'",
                    sprite.atlas,
                    sprite.region
                ));
            };
            entity.insert(Sprite {
                atlas_key: Arc::from(sprite.atlas.clone()),
                region: Arc::clone(region_name),
                region_id: info.id,
                uv: info.uv,
            });
        }

        if let Some(mesh) = data.mesh.as_ref() {
            entity.insert(MeshRef { key: mesh.key.clone() });
            let surface = MeshSurface {
                material: mesh.material.clone(),
                lighting: MeshLighting::from(mesh.lighting.clone()),
            };
            entity.insert(surface);
        }

        if let Some(body) = body_handle {
            entity.insert(RapierBody { handle: body });
        }
        if let Some(collider) = collider_handle {
            entity.insert(RapierCollider { handle: collider });
        }

        let entity_id = entity.id();
        drop(entity);

        if let Some(collider) = collider_handle {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.register_collider_entity(collider, entity_id);
        }

        if let Some(sprite) = data.sprite.as_ref().and_then(|sprite_data| sprite_data.animation.as_ref()) {
            if !self.set_sprite_timeline(entity_id, assets, Some(&sprite.timeline)) {
                eprintln!(
                    "[scene] sprite animation '{}' was not found for atlas '{}'",
                    sprite.timeline,
                    data.sprite.as_ref().map(|s| s.atlas.as_str()).unwrap_or_default()
                );
            } else {
                self.set_sprite_animation_speed(entity_id, sprite.speed);
                self.set_sprite_animation_start_offset(entity_id, sprite.start_offset);
                self.set_sprite_animation_random_start(entity_id, sprite.random_start);
                self.set_sprite_animation_group(entity_id, sprite.group.as_deref());
                if let Some(mode_str) = sprite.loop_mode.as_ref() {
                    let mode = SpriteAnimationLoopMode::from_str(mode_str);
                    self.set_sprite_animation_loop_mode(entity_id, mode);
                } else {
                    self.set_sprite_animation_looped(entity_id, sprite.looped);
                }
                self.set_sprite_animation_playing(entity_id, sprite.playing);
            }
        }

        if data.sprite.is_some() {
            if let Some(sprite) = self.world.get::<Sprite>(entity_id) {
                self.emit(GameEvent::SpriteSpawned {
                    entity: entity_id,
                    atlas: sprite.atlas_key.to_string(),
                    region: sprite.region.to_string(),
                });
            }
        }

        Ok(entity_id)
    }

    fn collect_scene_entity(
        &mut self,
        entity: Entity,
        parent_index: Option<usize>,
        parent_id: Option<SceneEntityId>,
        out: &mut Vec<SceneEntity>,
    ) {
        if self.world.get::<Transform>(entity).is_none() {
            return;
        }
        if self.world.get::<Particle>(entity).is_some() {
            return;
        }

        let entity_id = self.ensure_scene_entity_tag(entity);
        let transform = *self.world.get::<Transform>(entity).unwrap();
        let mesh_surface = self.world.get::<MeshSurface>(entity).cloned();
        let scene_entity = SceneEntity {
            id: entity_id.clone(),
            name: None,
            transform: TransformData::from_components(
                transform.translation,
                transform.rotation,
                transform.scale,
            ),
            sprite: self
                .world
                .get::<Sprite>(entity)
                .map(|sprite| (sprite.atlas_key.to_string(), sprite.region.to_string()))
                .map(|(atlas, region)| {
                    let animation =
                        self.world.get::<SpriteAnimation>(entity).map(|anim| SpriteAnimationData {
                            timeline: anim.timeline.as_ref().to_string(),
                            speed: anim.speed,
                            looped: anim.looped,
                            playing: anim.playing,
                            loop_mode: Some(anim.mode.as_str().to_string()),
                            start_offset: anim.start_offset,
                            random_start: anim.random_start,
                            group: anim.group.clone(),
                        });
                    SpriteData { atlas, region, animation }
                }),
            transform3d: self
                .world
                .get::<Transform3D>(entity)
                .map(|t| Transform3DData::from_components(t.translation, t.rotation, t.scale)),
            mesh: self.world.get::<MeshRef>(entity).map(|mesh| {
                let (material, lighting) = if let Some(surface) = mesh_surface {
                    (surface.material.clone(), MeshLightingData::from(&surface.lighting))
                } else {
                    (None, MeshLightingData::default())
                };
                MeshData { key: mesh.key.clone(), material, lighting }
            }),
            tint: self.world.get::<Tint>(entity).map(|t| ColorData::from(t.0)),
            velocity: self.world.get::<Velocity>(entity).map(|v| v.0.into()),
            mass: self.world.get::<Mass>(entity).map(|m| m.0),
            collider: self.world.get::<Aabb>(entity).map(|a| ColliderData { half_extents: a.half.into() }),
            particle_emitter: self.world.get::<ParticleEmitter>(entity).map(|emitter| ParticleEmitterData {
                rate: emitter.rate,
                spread: emitter.spread,
                speed: emitter.speed,
                lifetime: emitter.lifetime,
                start_color: emitter.start_color.into(),
                end_color: emitter.end_color.into(),
                start_size: emitter.start_size,
                end_size: emitter.end_size,
            }),
            orbit: self.world.get::<OrbitController>(entity).map(|orbit| OrbitControllerData {
                center: orbit.center.into(),
                angular_speed: orbit.angular_speed,
            }),
            spin: self.world.get::<Spin>(entity).map(|s| s.speed),
            parent_id: parent_id.clone(),
            parent: parent_index,
        };

        let current_index = out.len();
        out.push(scene_entity);

        if let Some(child_entities) = self.world.get::<Children>(entity).map(|children| children.0.clone()) {
            for child in child_entities {
                self.collect_scene_entity(child, Some(current_index), Some(entity_id.clone()), out);
            }
        }
    }

    fn clear_scene_entities(&mut self) {
        let boundary = {
            let rapier = self.world.resource::<RapierState>();
            rapier.boundary_entity()
        };
        let mut roots = Vec::new();
        {
            let mut query = self.world.query::<(Entity, Option<&Parent>)>();
            for (entity, parent) in query.iter(&self.world) {
                if parent.is_none() {
                    roots.push(entity);
                }
            }
        }
        for entity in roots {
            if entity == boundary {
                continue;
            }
            self.despawn_entity(entity);
        }
        self.world.resource_mut::<ParticleContacts>().pairs.clear();
    }
}
