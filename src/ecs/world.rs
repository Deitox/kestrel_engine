use super::*;
use crate::assets::AssetManager;
use crate::events::{EventBus, GameEvent};
use crate::mesh_registry::MeshRegistry;
use crate::scene::{
    ColliderData, ColorData, MeshData, MeshLightingData, OrbitControllerData, ParticleEmitterData, Scene,
    SceneDependencies, SceneEntity, SpriteData, Transform3DData, TransformData,
};
use anyhow::{anyhow, Context, Result};
use bevy_ecs::prelude::{Entity, Schedule, With, World};
use glam::{EulerRot, Mat4, Quat, Vec2, Vec3, Vec4};
use rand::Rng;
use rapier2d::prelude::{Rotation, Vector};
use std::borrow::Cow;
use std::path::Path;

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
        world.insert_resource(SpatialHash::new(0.25));
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

        let mut schedule_var = Schedule::default();
        schedule_var.add_systems((
            sys_apply_spin,
            sys_propagate_scene_transforms,
            sys_sync_world3d,
            sys_update_emitters,
            sys_update_particles,
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

    pub fn spawn_demo_scene(&mut self) -> Entity {
        let root = self
            .world
            .spawn((
                Transform { translation: Vec2::ZERO, rotation: 0.0, scale: Vec2::splat(0.8) },
                WorldTransform::default(),
                Spin { speed: 1.2 },
                Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed("checker") },
                Tint(Vec4::new(1.0, 1.0, 1.0, 0.2)),
            ))
            .id();
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
                Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed("checker") },
                Aabb { half: half_a },
                Velocity(velocity_a),
                Force::default(),
                Mass(1.0),
                RapierBody { handle: body_a },
                RapierCollider { handle: collider_a },
                OrbitController { center: orbit_center, angular_speed: orbit_speed_a },
            ))
            .id();
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
                Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed("redorb") },
                Aabb { half: half_b },
                Velocity(velocity_b),
                Force::default(),
                Mass(1.0),
                RapierBody { handle: body_b },
                RapierCollider { handle: collider_b },
                OrbitController { center: orbit_center, angular_speed: orbit_speed_b },
            ))
            .id();
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
                Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed("bluebox") },
                Aabb { half: half_c },
                Velocity(velocity_c),
                Force::default(),
                Mass(1.0),
                RapierBody { handle: body_c },
                RapierCollider { handle: collider_c },
                OrbitController { center: orbit_center, angular_speed: orbit_speed_c },
            ))
            .id();
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
                    Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed(rname) },
                    Aabb { half },
                    Velocity(vel),
                    Force::default(),
                    Mass(0.8),
                    RapierBody { handle: body_handle },
                    RapierCollider { handle: collider_handle },
                ))
                .id();
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
        self.world
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
            .id()
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
        if !assets.atlas_region_exists(atlas, region) {
            return Err(anyhow!("Region '{region}' not found in atlas '{atlas}'"));
        }
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
                Sprite { atlas_key: Cow::Owned(atlas.to_string()), region: Cow::Owned(region.to_string()) },
                Aabb { half },
                Velocity(velocity),
                Force::default(),
                Mass(1.0),
                RapierBody { handle: body_handle },
                RapierCollider { handle: collider_handle },
            ))
            .id();
        {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.register_collider_entity(collider_handle, entity);
        }
        self.emit(GameEvent::SpriteSpawned { entity, atlas: atlas.to_string(), region: region.to_string() });
        Ok(entity)
    }

    pub fn spawn_mesh_entity(&mut self, mesh_key: &str, translation: Vec3, scale: Vec3) -> Entity {
        let transform3d = Transform3D { translation, rotation: Quat::IDENTITY, scale };
        let world3d =
            WorldTransform3D(Mat4::from_scale_rotation_translation(scale, Quat::IDENTITY, translation));
        self.world
            .spawn((
                Transform::default(),
                WorldTransform::default(),
                transform3d,
                world3d,
                MeshRef { key: mesh_key.to_string() },
                MeshSurface::default(),
            ))
            .id()
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
    pub fn set_sprite_region(&mut self, entity: Entity, assets: &AssetManager, region: &str) -> bool {
        if let Some(mut sprite) = self.world.get_mut::<Sprite>(entity) {
            let atlas = sprite.atlas_key.to_string();
            if !assets.atlas_region_exists(&atlas, region) {
                return false;
            }
            sprite.region = Cow::Owned(region.to_string());
            true
        } else {
            false
        }
    }
    pub fn collect_sprite_instances(&mut self, assets: &AssetManager) -> Result<Vec<SpriteInstance>> {
        let mut out = Vec::new();
        let mut q = self.world.query::<(&WorldTransform, &Sprite, Option<&Tint>)>();
        for (wt, sprite, tint) in q.iter(&self.world) {
            let atlas_key = sprite.atlas_key.as_ref();
            let uv_rect = assets.atlas_region_uv(atlas_key, sprite.region.as_ref()).with_context(|| {
                format!("Collecting sprite instance for atlas '{}' region '{}'", atlas_key, sprite.region)
            })?;
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
    pub fn entity_info(&self, entity: Entity) -> Option<EntityInfo> {
        let transform = self.world.get::<Transform>(entity)?;
        let world_transform = self.world.get::<WorldTransform>(entity)?;
        let translation = Vec2::new(world_transform.0.w_axis.x, world_transform.0.w_axis.y);
        let velocity = self.world.get::<Velocity>(entity).map(|v| v.0);
        let sprite = self.world.get::<Sprite>(entity).map(|sprite| SpriteInfo {
            atlas: sprite.atlas_key.to_string(),
            region: sprite.region.to_string(),
        });
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
            translation,
            rotation: transform.rotation,
            scale: transform.scale,
            velocity,
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
        for entity_data in &scene.entities {
            let entity = self.spawn_scene_entity(entity_data, assets)?;
            entity_map.push(entity);
        }
        for (index, entity_data) in scene.entities.iter().enumerate() {
            if let Some(parent_index) = entity_data.parent {
                let parent_entity = *entity_map
                    .get(parent_index)
                    .ok_or_else(|| anyhow!("Scene entity parent index {parent_index} out of bounds"))?;
                let child_entity = entity_map[index];
                self.world.entity_mut(child_entity).insert(Parent(parent_entity));
                if let Some(mut children) = self.world.get_mut::<Children>(parent_entity) {
                    if !children.0.contains(&child_entity) {
                        children.0.push(child_entity);
                    }
                } else {
                    self.world.entity_mut(parent_entity).insert(Children(vec![child_entity]));
                }
            }
        }
        Ok(())
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
            self.collect_scene_entity(root, None, &mut scene.entities);
        }
        scene.dependencies =
            SceneDependencies::from_entities(&scene.entities, assets, mesh_source, material_source);
        scene
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

        let mut sprite_event = None;
        if let Some(sprite) = data.sprite.as_ref() {
            if !assets.atlas_region_exists(&sprite.atlas, &sprite.region) {
                return Err(anyhow!(
                    "Scene references unknown atlas region '{}:{}'",
                    sprite.atlas,
                    sprite.region
                ));
            }
            entity.insert(Sprite {
                atlas_key: Cow::Owned(sprite.atlas.clone()),
                region: Cow::Owned(sprite.region.clone()),
            });
            sprite_event = Some((sprite.atlas.clone(), sprite.region.clone()));
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

        if let Some((atlas, region)) = sprite_event {
            self.emit(GameEvent::SpriteSpawned { entity: entity_id, atlas, region });
        }

        Ok(entity_id)
    }

    fn collect_scene_entity(&self, entity: Entity, parent_index: Option<usize>, out: &mut Vec<SceneEntity>) {
        if self.world.get::<Transform>(entity).is_none() {
            return;
        }
        if self.world.get::<Particle>(entity).is_some() {
            return;
        }

        let transform = *self.world.get::<Transform>(entity).unwrap();
        let mesh_surface = self.world.get::<MeshSurface>(entity);
        let scene_entity = SceneEntity {
            name: None,
            transform: TransformData::from_components(
                transform.translation,
                transform.rotation,
                transform.scale,
            ),
            sprite: self.world.get::<Sprite>(entity).map(|sprite| SpriteData {
                atlas: sprite.atlas_key.to_string(),
                region: sprite.region.to_string(),
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
            parent: parent_index,
        };

        let current_index = out.len();
        out.push(scene_entity);

        if let Some(children) = self.world.get::<Children>(entity) {
            for &child in &children.0 {
                self.collect_scene_entity(child, Some(current_index), out);
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
