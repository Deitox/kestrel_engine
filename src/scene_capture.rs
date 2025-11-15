use crate::scene::{
    ColorData, Scene, SceneCamera2D, SceneDependencies, SceneEntity, SceneEnvironment, SceneLightingData,
    SceneViewportMode, SpriteAnimationData, SpriteData, TransformClipData, Vec2Data, Vec3Data,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::Path;

/// Deterministic snapshot of a scene used for docs/tests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneCaptureSummary {
    pub metadata: SceneCaptureMetadata,
    pub dependencies: SceneCaptureDependencies,
    pub entities: Vec<SceneCaptureEntity>,
}

impl SceneCaptureSummary {
    pub fn from_scene(scene: &Scene) -> Self {
        Self {
            metadata: SceneCaptureMetadata::from_scene(scene),
            dependencies: SceneCaptureDependencies::from_dependencies(&scene.dependencies),
            entities: capture_entities(&scene.entities),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneCaptureMetadata {
    pub viewport: SceneViewportMode,
    pub camera2d: Option<SceneCaptureCamera2D>,
    pub environment: Option<SceneCaptureEnvironment>,
    pub lighting: Option<SceneCaptureLighting>,
}

impl SceneCaptureMetadata {
    fn from_scene(scene: &Scene) -> Self {
        Self {
            viewport: scene.metadata.viewport,
            camera2d: scene.metadata.camera2d.as_ref().map(SceneCaptureCamera2D::from),
            environment: scene.metadata.environment.as_ref().map(SceneCaptureEnvironment::from),
            lighting: scene.metadata.lighting.as_ref().map(SceneCaptureLighting::from),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneCaptureCamera2D {
    pub position: [f32; 2],
    pub zoom: f32,
}

impl From<&SceneCamera2D> for SceneCaptureCamera2D {
    fn from(camera: &SceneCamera2D) -> Self {
        Self { position: vec2_to_array(&camera.position), zoom: camera.zoom }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneCaptureEnvironment {
    pub key: String,
    pub intensity: f32,
}

impl From<&SceneEnvironment> for SceneCaptureEnvironment {
    fn from(env: &SceneEnvironment) -> Self {
        Self { key: env.key.clone(), intensity: env.intensity }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneCaptureLighting {
    pub direction: [f32; 3],
    pub color: [f32; 3],
    pub ambient: [f32; 3],
    pub exposure: f32,
}

impl From<&SceneLightingData> for SceneCaptureLighting {
    fn from(light: &SceneLightingData) -> Self {
        Self {
            direction: vec3_to_array(&light.direction),
            color: vec3_to_array(&light.color),
            ambient: vec3_to_array(&light.ambient),
            exposure: light.exposure,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneCaptureDependencies {
    pub atlases: Vec<SceneCaptureDependencyRef>,
    pub clips: Vec<SceneCaptureDependencyRef>,
    pub meshes: Vec<SceneCaptureDependencyRef>,
    pub materials: Vec<SceneCaptureDependencyRef>,
    pub environments: Vec<SceneCaptureDependencyRef>,
}

impl SceneCaptureDependencies {
    fn from_dependencies(deps: &SceneDependencies) -> Self {
        fn collect<'a, T: 'a>(
            iter: impl Iterator<Item = T>,
            mut convert: impl FnMut(T) -> SceneCaptureDependencyRef,
        ) -> Vec<SceneCaptureDependencyRef> {
            let mut out: Vec<_> = iter.map(&mut convert).collect();
            out.sort_by(|a, b| a.key.cmp(&b.key));
            out
        }

        Self {
            atlases: collect(deps.atlas_dependencies(), |dep| SceneCaptureDependencyRef {
                key: dep.key().to_string(),
                path: dep.path().map(|p| p.to_string()),
            }),
            clips: collect(deps.clip_dependencies(), |dep| SceneCaptureDependencyRef {
                key: dep.key().to_string(),
                path: dep.path().map(|p| p.to_string()),
            }),
            meshes: collect(deps.mesh_dependencies(), |dep| SceneCaptureDependencyRef {
                key: dep.key().to_string(),
                path: dep.path().map(|p| p.to_string()),
            }),
            materials: collect(deps.material_dependencies(), |dep| SceneCaptureDependencyRef {
                key: dep.key().to_string(),
                path: dep.path().map(|p| p.to_string()),
            }),
            environments: collect(deps.environment_dependencies(), |dep| SceneCaptureDependencyRef {
                key: dep.key().to_string(),
                path: dep.path().map(|p| p.to_string()),
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneCaptureDependencyRef {
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneCaptureEntity {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub transform: SceneCaptureTransform,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sprite: Option<SceneCaptureSprite>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transform_clip: Option<SceneCaptureTransformClip>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tint: Option<[f32; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spin: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneCaptureTransform {
    pub translation: [f32; 2],
    pub rotation: f32,
    pub scale: [f32; 2],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneCaptureSprite {
    pub atlas: String,
    pub region: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub animation: Option<SceneCaptureSpriteAnimation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneCaptureSpriteAnimation {
    pub timeline: String,
    pub speed: f32,
    pub looped: bool,
    pub playing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneCaptureTransformClip {
    pub clip_key: String,
    pub playing: bool,
    pub looped: bool,
    pub speed: f32,
    pub time: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub mask: SceneCaptureClipMask,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneCaptureClipMask {
    pub translation: bool,
    pub rotation: bool,
    pub scale: bool,
    pub tint: bool,
}

fn capture_entities(entities: &[SceneEntity]) -> Vec<SceneCaptureEntity> {
    let mut list: Vec<_> = entities.iter().map(SceneCaptureEntity::from).collect();
    list.sort_by(|a, b| {
        let a_key = a.name.as_deref().unwrap_or(&a.id);
        let b_key = b.name.as_deref().unwrap_or(&b.id);
        match a_key.cmp(b_key) {
            Ordering::Equal => a.id.cmp(&b.id),
            other => other,
        }
    });
    list
}

impl From<&SceneEntity> for SceneCaptureEntity {
    fn from(entity: &SceneEntity) -> Self {
        Self {
            id: entity.id.as_str().to_string(),
            name: entity.name.clone(),
            transform: SceneCaptureTransform {
                translation: vec2_to_array(&entity.transform.translation),
                rotation: entity.transform.rotation,
                scale: vec2_to_array(&entity.transform.scale),
            },
            sprite: entity.sprite.as_ref().map(SceneCaptureSprite::from),
            transform_clip: entity.transform_clip.as_ref().map(SceneCaptureTransformClip::from),
            tint: entity.tint.as_ref().map(color_to_array),
            spin: entity.spin,
        }
    }
}

impl From<&SpriteData> for SceneCaptureSprite {
    fn from(sprite: &SpriteData) -> Self {
        Self {
            atlas: sprite.atlas.clone(),
            region: sprite.region.clone(),
            animation: sprite.animation.as_ref().map(SceneCaptureSpriteAnimation::from),
        }
    }
}

impl From<&SpriteAnimationData> for SceneCaptureSpriteAnimation {
    fn from(anim: &SpriteAnimationData) -> Self {
        Self {
            timeline: anim.timeline.clone(),
            speed: anim.speed,
            looped: anim.looped,
            playing: anim.playing,
            loop_mode: anim.loop_mode.clone(),
            group: anim.group.clone(),
        }
    }
}

impl From<&TransformClipData> for SceneCaptureTransformClip {
    fn from(clip: &TransformClipData) -> Self {
        Self {
            clip_key: clip.clip_key.clone(),
            playing: clip.playing,
            looped: clip.looped,
            speed: clip.speed,
            time: clip.time,
            group: clip.group.clone(),
            mask: SceneCaptureClipMask {
                translation: clip.apply_translation,
                rotation: clip.apply_rotation,
                scale: clip.apply_scale,
                tint: clip.apply_tint,
            },
        }
    }
}

fn vec2_to_array(vec: &Vec2Data) -> [f32; 2] {
    [vec.x, vec.y]
}

fn vec3_to_array(vec: &Vec3Data) -> [f32; 3] {
    [vec.x, vec.y, vec.z]
}

fn color_to_array(color: &ColorData) -> [f32; 4] {
    [color.r, color.g, color.b, color.a]
}

pub fn capture_scene_from_path(path: impl AsRef<Path>) -> Result<SceneCaptureSummary> {
    let scene = Scene::load_from_path(path)?;
    Ok(SceneCaptureSummary::from_scene(&scene))
}
