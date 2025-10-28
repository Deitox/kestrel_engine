use crate::assets::AssetManager;
#[cfg(feature = "binary_scene")]
use anyhow::anyhow;
use anyhow::{bail, Context, Result};
use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::Path;
use uuid::Uuid;

const BINARY_SCENE_MAGIC: [u8; 4] = *b"KSCN";
#[cfg(feature = "binary_scene")]
const BINARY_SCENE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scene {
    #[serde(default)]
    pub metadata: SceneMetadata,
    #[serde(default)]
    pub dependencies: SceneDependencies,
    #[serde(default)]
    pub entities: Vec<SceneEntity>,
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            metadata: SceneMetadata::default(),
            dependencies: SceneDependencies::default(),
            entities: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneMetadata {
    #[serde(default)]
    pub viewport: SceneViewportMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera2d: Option<SceneCamera2D>,
    #[serde(default)]
    pub camera_bookmarks: Vec<SceneCameraBookmark>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_camera_bookmark: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_follow_entity: Option<SceneEntityId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_camera: Option<ScenePreviewCamera>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lighting: Option<SceneLightingData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<SceneEnvironment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneCamera2D {
    pub position: Vec2Data,
    pub zoom: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneCameraBookmark {
    pub name: String,
    pub position: Vec2Data,
    pub zoom: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenePreviewCamera {
    #[serde(default)]
    pub mode: ScenePreviewCameraMode,
    #[serde(default)]
    pub orbit: SceneOrbitCamera,
    #[serde(default)]
    pub freefly: SceneFreeflyCamera,
    #[serde(default)]
    pub frustum_lock: bool,
    #[serde(default)]
    pub frustum_focus: Vec3Data,
    #[serde(default)]
    pub frustum_distance: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SceneEntityId(String);

impl SceneEntityId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Default for SceneEntityId {
    fn default() -> Self {
        SceneEntityId::new()
    }
}

fn default_light_direction() -> Vec3Data {
    let dir = glam::Vec3::new(0.4, 0.8, 0.35).normalize();
    Vec3Data::from(dir)
}

fn default_light_color() -> Vec3Data {
    Vec3Data { x: 1.05, y: 0.98, z: 0.92 }
}

fn default_light_ambient() -> Vec3Data {
    Vec3Data { x: 0.03, y: 0.03, z: 0.03 }
}

const fn default_light_exposure() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneLightingData {
    #[serde(default = "default_light_direction")]
    pub direction: Vec3Data,
    #[serde(default = "default_light_color")]
    pub color: Vec3Data,
    #[serde(default = "default_light_ambient")]
    pub ambient: Vec3Data,
    #[serde(default = "default_light_exposure")]
    pub exposure: f32,
    #[serde(default)]
    pub shadow: SceneShadowData,
}

impl Default for SceneLightingData {
    fn default() -> Self {
        Self {
            direction: default_light_direction(),
            color: default_light_color(),
            ambient: default_light_ambient(),
            exposure: default_light_exposure(),
            shadow: SceneShadowData::default(),
        }
    }
}

impl SceneLightingData {
    pub fn components(&self) -> (glam::Vec3, glam::Vec3, glam::Vec3, f32, SceneShadowData) {
        (
            self.direction.clone().into(),
            self.color.clone().into(),
            self.ambient.clone().into(),
            self.exposure,
            self.shadow.clone(),
        )
    }
}

const fn default_environment_intensity() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneEnvironment {
    pub key: String,
    #[serde(default = "default_environment_intensity")]
    pub intensity: f32,
}

impl SceneEnvironment {
    pub fn new(key: String, intensity: f32) -> Self {
        Self { key, intensity }
    }
}

fn default_shadow_distance() -> f32 {
    35.0
}

fn default_shadow_bias() -> f32 {
    0.002
}

fn default_shadow_strength() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneShadowData {
    #[serde(default = "default_shadow_distance")]
    pub distance: f32,
    #[serde(default = "default_shadow_bias")]
    pub bias: f32,
    #[serde(default = "default_shadow_strength")]
    pub strength: f32,
}

impl Default for SceneShadowData {
    fn default() -> Self {
        Self {
            distance: default_shadow_distance(),
            bias: default_shadow_bias(),
            strength: default_shadow_strength(),
        }
    }
}

impl Default for ScenePreviewCamera {
    fn default() -> Self {
        Self {
            mode: ScenePreviewCameraMode::Orbit,
            orbit: SceneOrbitCamera::default(),
            freefly: SceneFreeflyCamera::default(),
            frustum_lock: false,
            frustum_focus: Vec3Data { x: 0.0, y: 0.0, z: 0.0 },
            frustum_distance: 5.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneOrbitCamera {
    pub target: Vec3Data,
    pub radius: f32,
    pub yaw: f32,
    pub pitch: f32,
}

impl Default for SceneOrbitCamera {
    fn default() -> Self {
        Self { target: Vec3Data { x: 0.0, y: 0.0, z: 0.0 }, radius: 5.0, yaw: 0.0, pitch: 0.0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneFreeflyCamera {
    pub position: Vec3Data,
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub speed: f32,
}

impl Default for SceneFreeflyCamera {
    fn default() -> Self {
        Self { position: Vec3Data { x: 0.0, y: 0.0, z: 5.0 }, yaw: 0.0, pitch: 0.0, roll: 0.0, speed: 4.0 }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ScenePreviewCameraMode {
    Disabled,
    Orbit,
    Freefly,
}

impl Default for ScenePreviewCameraMode {
    fn default() -> Self {
        ScenePreviewCameraMode::Orbit
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SceneViewportMode {
    Ortho2D,
    Perspective3D,
}

impl Default for SceneViewportMode {
    fn default() -> Self {
        SceneViewportMode::Ortho2D
    }
}

#[derive(Debug, Clone)]
pub struct AtlasDependency {
    key: String,
    path: Option<String>,
}

impl AtlasDependency {
    pub fn new(key: String, path: Option<String>) -> Self {
        Self { key, path }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }
}

pub struct AtlasDependencyView<'a> {
    key: &'a str,
    path: Option<&'a str>,
}

impl<'a> AtlasDependencyView<'a> {
    fn new(key: &'a str, path: Option<&'a str>) -> Self {
        Self { key, path }
    }

    pub fn key(&self) -> &str {
        self.key
    }

    pub fn path(&self) -> Option<&str> {
        self.path
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum AtlasDependencyRepr {
    Key(String),
    Detailed {
        key: String,
        #[serde(default)]
        path: Option<String>,
    },
}

impl From<AtlasDependency> for AtlasDependencyRepr {
    fn from(dep: AtlasDependency) -> Self {
        if let Some(path) = dep.path {
            AtlasDependencyRepr::Detailed { key: dep.key, path: Some(path) }
        } else {
            AtlasDependencyRepr::Key(dep.key)
        }
    }
}

impl From<AtlasDependencyRepr> for AtlasDependency {
    fn from(repr: AtlasDependencyRepr) -> Self {
        match repr {
            AtlasDependencyRepr::Key(key) => AtlasDependency::new(key, None),
            AtlasDependencyRepr::Detailed { key, path } => AtlasDependency::new(key, path),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MeshDependency {
    key: String,
    path: Option<String>,
}

impl MeshDependency {
    pub fn new(key: String, path: Option<String>) -> Self {
        Self { key, path }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }
}

pub struct MeshDependencyView<'a> {
    key: &'a str,
    path: Option<&'a str>,
}

impl<'a> MeshDependencyView<'a> {
    fn new(key: &'a str, path: Option<&'a str>) -> Self {
        Self { key, path }
    }

    pub fn key(&self) -> &str {
        self.key
    }

    pub fn path(&self) -> Option<&str> {
        self.path
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum MeshDependencyRepr {
    Key(String),
    Detailed {
        key: String,
        #[serde(default)]
        path: Option<String>,
    },
}

impl From<MeshDependency> for MeshDependencyRepr {
    fn from(dep: MeshDependency) -> Self {
        if let Some(path) = dep.path {
            MeshDependencyRepr::Detailed { key: dep.key, path: Some(path) }
        } else {
            MeshDependencyRepr::Key(dep.key)
        }
    }
}

impl From<MeshDependencyRepr> for MeshDependency {
    fn from(repr: MeshDependencyRepr) -> Self {
        match repr {
            MeshDependencyRepr::Key(key) => MeshDependency::new(key, None),
            MeshDependencyRepr::Detailed { key, path } => MeshDependency::new(key, path),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MaterialDependency {
    key: String,
    path: Option<String>,
}

impl MaterialDependency {
    pub fn new(key: String, path: Option<String>) -> Self {
        Self { key, path }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }
}

pub struct MaterialDependencyView<'a> {
    key: &'a str,
    path: Option<&'a str>,
}

impl<'a> MaterialDependencyView<'a> {
    fn new(key: &'a str, path: Option<&'a str>) -> Self {
        Self { key, path }
    }

    pub fn key(&self) -> &str {
        self.key
    }

    pub fn path(&self) -> Option<&str> {
        self.path
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum MaterialDependencyRepr {
    Key(String),
    Detailed {
        key: String,
        #[serde(default)]
        path: Option<String>,
    },
}

impl From<MaterialDependency> for MaterialDependencyRepr {
    fn from(dep: MaterialDependency) -> Self {
        if let Some(path) = dep.path {
            MaterialDependencyRepr::Detailed { key: dep.key, path: Some(path) }
        } else {
            MaterialDependencyRepr::Key(dep.key)
        }
    }
}

impl From<MaterialDependencyRepr> for MaterialDependency {
    fn from(repr: MaterialDependencyRepr) -> Self {
        match repr {
            MaterialDependencyRepr::Key(key) => MaterialDependency::new(key, None),
            MaterialDependencyRepr::Detailed { key, path } => MaterialDependency::new(key, path),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EnvironmentDependency {
    key: String,
    path: Option<String>,
}

impl EnvironmentDependency {
    pub fn new(key: String, path: Option<String>) -> Self {
        Self { key, path }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }
}

pub struct EnvironmentDependencyView<'a> {
    key: &'a str,
    path: Option<&'a str>,
}

impl<'a> EnvironmentDependencyView<'a> {
    fn new(key: &'a str, path: Option<&'a str>) -> Self {
        Self { key, path }
    }

    pub fn key(&self) -> &str {
        self.key
    }

    pub fn path(&self) -> Option<&str> {
        self.path
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum EnvironmentDependencyRepr {
    Key(String),
    Detailed {
        key: String,
        #[serde(default)]
        path: Option<String>,
    },
}

impl From<EnvironmentDependency> for EnvironmentDependencyRepr {
    fn from(dep: EnvironmentDependency) -> Self {
        if let Some(path) = dep.path {
            EnvironmentDependencyRepr::Detailed { key: dep.key, path: Some(path) }
        } else {
            EnvironmentDependencyRepr::Key(dep.key)
        }
    }
}

impl From<EnvironmentDependencyRepr> for EnvironmentDependency {
    fn from(repr: EnvironmentDependencyRepr) -> Self {
        match repr {
            EnvironmentDependencyRepr::Key(key) => EnvironmentDependency::new(key, None),
            EnvironmentDependencyRepr::Detailed { key, path } => EnvironmentDependency::new(key, path),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneDependencies {
    #[serde(default)]
    atlases: Vec<AtlasDependencyRepr>,
    #[serde(default)]
    meshes: Vec<MeshDependencyRepr>,
    #[serde(default)]
    materials: Vec<MaterialDependencyRepr>,
    #[serde(default)]
    environments: Vec<EnvironmentDependencyRepr>,
}

impl SceneDependencies {
    pub fn from_entities<F, G>(
        entities: &[SceneEntity],
        assets: &AssetManager,
        mesh_source: F,
        material_source: G,
    ) -> Self
    where
        F: Fn(&str) -> Option<String>,
        G: Fn(&str) -> Option<String>,
    {
        let mut set = BTreeSet::new();
        for entity in entities {
            if let Some(sprite) = &entity.sprite {
                set.insert(sprite.atlas.clone());
            }
        }
        let mut deps = SceneDependencies {
            atlases: set
                .into_iter()
                .map(|key| {
                    let path = assets.atlas_source(&key).map(|p| p.to_string());
                    AtlasDependencyRepr::from(AtlasDependency::new(key, path))
                })
                .collect(),
            meshes: Vec::new(),
            materials: Vec::new(),
            environments: Vec::new(),
        };
        let mut mesh_set = BTreeSet::new();
        for entity in entities {
            if let Some(mesh) = &entity.mesh {
                mesh_set.insert(mesh.key.clone());
            }
        }
        deps.meshes = mesh_set
            .into_iter()
            .map(|key| {
                let path = mesh_source(&key);
                MeshDependencyRepr::from(MeshDependency::new(key, path))
            })
            .collect();
        let mut material_set = BTreeSet::new();
        for entity in entities {
            if let Some(mesh) = &entity.mesh {
                if let Some(material) = &mesh.material {
                    material_set.insert(material.clone());
                }
            }
        }
        deps.materials = material_set
            .into_iter()
            .map(|key| {
                let path = material_source(&key);
                MaterialDependencyRepr::from(MaterialDependency::new(key, path))
            })
            .collect();
        deps.normalize();
        deps
    }

    pub fn contains_atlas(&self, key: &str) -> bool {
        self.atlas_dependencies().any(|dep| dep.key() == key)
    }

    pub fn atlas_dependencies(&self) -> impl Iterator<Item = AtlasDependencyView<'_>> {
        self.atlases.iter().map(|repr| match repr {
            AtlasDependencyRepr::Key(key) => AtlasDependencyView::new(key, None),
            AtlasDependencyRepr::Detailed { key, path } => AtlasDependencyView::new(key, path.as_deref()),
        })
    }

    pub fn contains_mesh(&self, key: &str) -> bool {
        self.mesh_dependencies().any(|dep| dep.key() == key)
    }

    pub fn mesh_dependencies(&self) -> impl Iterator<Item = MeshDependencyView<'_>> {
        self.meshes.iter().map(|repr| match repr {
            MeshDependencyRepr::Key(key) => MeshDependencyView::new(key, None),
            MeshDependencyRepr::Detailed { key, path } => MeshDependencyView::new(key, path.as_deref()),
        })
    }

    pub fn material_dependencies(&self) -> impl Iterator<Item = MaterialDependencyView<'_>> {
        self.materials.iter().map(|repr| match repr {
            MaterialDependencyRepr::Key(key) => MaterialDependencyView::new(key, None),
            MaterialDependencyRepr::Detailed { key, path } => {
                MaterialDependencyView::new(key, path.as_deref())
            }
        })
    }

    pub fn contains_material(&self, key: &str) -> bool {
        self.material_dependencies().any(|dep| dep.key() == key)
    }

    pub fn environment_dependencies(&self) -> impl Iterator<Item = EnvironmentDependencyView<'_>> {
        self.environments.iter().map(|repr| match repr {
            EnvironmentDependencyRepr::Key(key) => EnvironmentDependencyView::new(key, None),
            EnvironmentDependencyRepr::Detailed { key, path } => {
                EnvironmentDependencyView::new(key, path.as_deref())
            }
        })
    }

    pub fn environment_dependency(&self) -> Option<EnvironmentDependencyView<'_>> {
        self.environment_dependencies().next()
    }

    pub fn contains_environment(&self, key: &str) -> bool {
        self.environment_dependencies().any(|dep| dep.key() == key)
    }

    pub fn set_environment_dependency(&mut self, dependency: Option<EnvironmentDependency>) {
        self.environments.clear();
        if let Some(dep) = dependency {
            self.environments.push(EnvironmentDependencyRepr::from(dep));
        }
    }

    pub fn fill_mesh_sources<F>(&mut self, mut f: F)
    where
        F: FnMut(&str) -> Option<String>,
    {
        for repr in &mut self.meshes {
            match repr {
                MeshDependencyRepr::Key(key) => {
                    if let Some(path) = f(key) {
                        *repr = MeshDependencyRepr::Detailed { key: key.clone(), path: Some(path) };
                    }
                }
                MeshDependencyRepr::Detailed { key, path } => {
                    if path.is_none() {
                        if let Some(new_path) = f(key) {
                            *path = Some(new_path);
                        }
                    }
                }
            }
        }
    }

    pub fn fill_material_sources<F>(&mut self, mut f: F)
    where
        F: FnMut(&str) -> Option<String>,
    {
        for repr in &mut self.materials {
            match repr {
                MaterialDependencyRepr::Key(key) => {
                    if let Some(path) = f(key) {
                        *repr = MaterialDependencyRepr::Detailed { key: key.clone(), path: Some(path) };
                    }
                }
                MaterialDependencyRepr::Detailed { key, path } => {
                    if path.is_none() {
                        if let Some(new_path) = f(key) {
                            *path = Some(new_path);
                        }
                    }
                }
            }
        }
    }

    pub fn subset_for_entities(
        &self,
        entities: &[SceneEntity],
        environment: Option<&SceneEnvironment>,
    ) -> Self {
        let mut atlas_keys = BTreeSet::new();
        let mut mesh_keys = BTreeSet::new();
        let mut material_keys = BTreeSet::new();
        for entity in entities {
            if let Some(sprite) = &entity.sprite {
                atlas_keys.insert(sprite.atlas.clone());
            }
            if let Some(mesh) = &entity.mesh {
                mesh_keys.insert(mesh.key.clone());
                if let Some(material) = &mesh.material {
                    material_keys.insert(material.clone());
                }
            }
        }

        let atlas_lookup: HashMap<_, _> = self
            .atlases
            .iter()
            .cloned()
            .map(|repr| {
                let dep: AtlasDependency = repr.into();
                (dep.key().to_string(), dep)
            })
            .collect();
        let mesh_lookup: HashMap<_, _> = self
            .meshes
            .iter()
            .cloned()
            .map(|repr| {
                let dep: MeshDependency = repr.into();
                (dep.key().to_string(), dep)
            })
            .collect();
        let material_lookup: HashMap<_, _> = self
            .materials
            .iter()
            .cloned()
            .map(|repr| {
                let dep: MaterialDependency = repr.into();
                (dep.key().to_string(), dep)
            })
            .collect();
        let environment_lookup: HashMap<_, _> = self
            .environments
            .iter()
            .cloned()
            .map(|repr| {
                let dep: EnvironmentDependency = repr.into();
                (dep.key().to_string(), dep)
            })
            .collect();

        let atlases = atlas_keys
            .into_iter()
            .map(|key| {
                let dep = atlas_lookup
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| AtlasDependency::new(key.clone(), None));
                AtlasDependencyRepr::from(dep)
            })
            .collect();
        let meshes = mesh_keys
            .into_iter()
            .map(|key| {
                let dep =
                    mesh_lookup.get(&key).cloned().unwrap_or_else(|| MeshDependency::new(key.clone(), None));
                MeshDependencyRepr::from(dep)
            })
            .collect();
        let materials = material_keys
            .into_iter()
            .map(|key| {
                let dep = material_lookup
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| MaterialDependency::new(key.clone(), None));
                MaterialDependencyRepr::from(dep)
            })
            .collect();

        let mut environments = Vec::new();
        if let Some(env) = environment {
            if let Some(dep) = environment_lookup.get(&env.key) {
                environments.push(EnvironmentDependencyRepr::from(dep.clone()));
            } else {
                environments
                    .push(EnvironmentDependencyRepr::from(EnvironmentDependency::new(env.key.clone(), None)));
            }
        }

        let mut subset = SceneDependencies { atlases, meshes, materials, environments };
        subset.normalize();
        subset
    }

    pub fn normalize(&mut self) {
        let mut map: BTreeMap<String, AtlasDependency> = BTreeMap::new();
        for repr in std::mem::take(&mut self.atlases) {
            let dep = AtlasDependency::from(repr);
            let key = dep.key().to_string();
            let path_opt = dep.path().map(|s| s.to_string());
            match map.entry(key) {
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    if entry.get().path.is_none() {
                        entry.get_mut().path = path_opt;
                    }
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    let key_clone = entry.key().clone();
                    entry.insert(AtlasDependency::new(key_clone, path_opt));
                }
            }
        }
        self.atlases = map.into_iter().map(|(_, dep)| AtlasDependencyRepr::from(dep)).collect();

        let mut mesh_map: BTreeMap<String, MeshDependency> = BTreeMap::new();
        for repr in std::mem::take(&mut self.meshes) {
            let dep = MeshDependency::from(repr);
            let key = dep.key().to_string();
            let path_opt = dep.path().map(|s| s.to_string());
            match mesh_map.entry(key) {
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    if entry.get().path.is_none() {
                        entry.get_mut().path = path_opt;
                    }
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    let key_clone = entry.key().clone();
                    entry.insert(MeshDependency::new(key_clone, path_opt));
                }
            }
        }
        self.meshes = mesh_map.into_iter().map(|(_, dep)| MeshDependencyRepr::from(dep)).collect();

        let mut material_map: BTreeMap<String, MaterialDependency> = BTreeMap::new();
        for repr in std::mem::take(&mut self.materials) {
            let dep = MaterialDependency::from(repr);
            let key = dep.key().to_string();
            let path_opt = dep.path().map(|s| s.to_string());
            match material_map.entry(key) {
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    if entry.get().path.is_none() {
                        entry.get_mut().path = path_opt;
                    }
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    let key_clone = entry.key().clone();
                    entry.insert(MaterialDependency::new(key_clone, path_opt));
                }
            }
        }
        self.materials = material_map.into_iter().map(|(_, dep)| MaterialDependencyRepr::from(dep)).collect();

        let mut environment_map: BTreeMap<String, EnvironmentDependency> = BTreeMap::new();
        for repr in std::mem::take(&mut self.environments) {
            let dep = EnvironmentDependency::from(repr);
            let key = dep.key().to_string();
            let path_opt = dep.path().map(|s| s.to_string());
            match environment_map.entry(key) {
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    if entry.get().path.is_none() {
                        entry.get_mut().path = path_opt;
                    }
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    let key_clone = entry.key().clone();
                    entry.insert(EnvironmentDependency::new(key_clone, path_opt));
                }
            }
        }
        self.environments =
            environment_map.into_iter().map(|(_, dep)| EnvironmentDependencyRepr::from(dep)).collect();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneEntity {
    #[serde(default)]
    pub id: SceneEntityId,
    #[serde(default)]
    pub name: Option<String>,
    pub transform: TransformData,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sprite: Option<SpriteData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform3d: Option<Transform3DData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<MeshData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tint: Option<ColorData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity: Option<Vec2Data>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mass: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collider: Option<ColliderData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub particle_emitter: Option<ParticleEmitterData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orbit: Option<OrbitControllerData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spin: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<SceneEntityId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformData {
    pub translation: Vec2Data,
    pub rotation: f32,
    pub scale: Vec2Data,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transform3DData {
    pub translation: Vec3Data,
    pub rotation: QuatData,
    pub scale: Vec3Data,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpriteData {
    pub atlas: String,
    pub region: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation: Option<SpriteAnimationData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpriteAnimationData {
    pub timeline: String,
    #[serde(default = "default_sprite_anim_speed")]
    pub speed: f32,
    #[serde(default = "default_sprite_anim_looped")]
    pub looped: bool,
    #[serde(default = "default_sprite_anim_playing")]
    pub playing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshData {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<String>,
    #[serde(default)]
    pub lighting: MeshLightingData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vec2Data {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Vec3Data {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuatData {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorData {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

const fn default_metallic() -> f32 {
    0.0
}

const fn default_roughness() -> f32 {
    0.5
}

fn default_base_color() -> Vec3Data {
    Vec3Data { x: 1.0, y: 1.0, z: 1.0 }
}

const fn default_receive_shadows() -> bool {
    true
}

const fn default_sprite_anim_looped() -> bool {
    true
}

const fn default_sprite_anim_playing() -> bool {
    true
}

const fn default_sprite_anim_speed() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshLightingData {
    #[serde(default)]
    pub cast_shadows: bool,
    #[serde(default = "default_receive_shadows")]
    pub receive_shadows: bool,
    #[serde(default = "default_base_color")]
    pub base_color: Vec3Data,
    #[serde(default = "default_metallic")]
    pub metallic: f32,
    #[serde(default = "default_roughness")]
    pub roughness: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emissive: Option<Vec3Data>,
}

impl Default for MeshLightingData {
    fn default() -> Self {
        Self {
            cast_shadows: false,
            receive_shadows: default_receive_shadows(),
            base_color: default_base_color(),
            metallic: default_metallic(),
            roughness: default_roughness(),
            emissive: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColliderData {
    pub half_extents: Vec2Data,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParticleEmitterData {
    pub rate: f32,
    pub spread: f32,
    pub speed: f32,
    pub lifetime: f32,
    pub start_color: ColorData,
    pub end_color: ColorData,
    pub start_size: f32,
    pub end_size: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrbitControllerData {
    pub center: Vec2Data,
    pub angular_speed: f32,
}

impl Scene {
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = fs::read(path).with_context(|| format!("Reading scene file {}", path.display()))?;
        if Self::is_binary_payload(&bytes) {
            #[cfg(feature = "binary_scene")]
            {
                let mut scene = Self::from_binary_bytes(&bytes)
                    .with_context(|| format!("Parsing binary scene {}", path.display()))?;
                scene.dependencies.normalize();
                scene.normalize_entities();
                return Ok(scene);
            }
            #[cfg(not(feature = "binary_scene"))]
            {
                bail!(
                    "Scene '{}' is binary (.kscene), but this build lacks the 'binary_scene' feature.",
                    path.display()
                );
            }
        }
        let mut scene = serde_json::from_slice::<Scene>(&bytes)
            .with_context(|| format!("Parsing scene file {}", path.display()))?;
        scene.dependencies.normalize();
        scene.normalize_entities();
        Ok(scene)
    }

    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Creating scene directory {}", parent.display()))?;
        }
        let normalized = self.normalized_clone();
        if Self::path_wants_binary(path) {
            #[cfg(feature = "binary_scene")]
            {
                let bytes = normalized.to_binary_bytes()?;
                fs::write(path, bytes).with_context(|| format!("Writing scene file {}", path.display()))?;
                return Ok(());
            }
            #[cfg(not(feature = "binary_scene"))]
            {
                bail!(
                    "Cannot write binary scene '{}': recompile with the 'binary_scene' feature enabled.",
                    path.display()
                );
            }
        }
        let json = serde_json::to_string_pretty(&normalized)?;
        fs::write(path, json.as_bytes()).with_context(|| format!("Writing scene file {}", path.display()))?;
        Ok(())
    }

    fn normalize_entities(&mut self) {
        let mut seen = HashSet::new();
        for entity in &mut self.entities {
            if entity.id.is_empty() || !seen.insert(entity.id.clone()) {
                let mut replacement = SceneEntityId::new();
                while !seen.insert(replacement.clone()) {
                    replacement = SceneEntityId::new();
                }
                entity.id = replacement;
            }
        }

        let inferred_parent_ids: Vec<Option<SceneEntityId>> = (0..self.entities.len())
            .map(|index| {
                self.entities[index]
                    .parent
                    .and_then(|parent_index| self.entities.get(parent_index).map(|parent| parent.id.clone()))
            })
            .collect();
        for (index, entity) in self.entities.iter_mut().enumerate() {
            if entity.parent_id.is_none() {
                if let Some(parent_id) = inferred_parent_ids[index].as_ref() {
                    entity.parent_id = Some(parent_id.clone());
                }
            }
        }

        let index_lookup: HashMap<String, usize> = self
            .entities
            .iter()
            .enumerate()
            .map(|(index, entity)| (entity.id.as_str().to_string(), index))
            .collect();
        let total_entities = self.entities.len();
        for entity in &mut self.entities {
            if let Some(parent_id) = entity.parent_id.as_ref() {
                if let Some(parent_index) = index_lookup.get(parent_id.as_str()) {
                    entity.parent = Some(*parent_index);
                } else {
                    entity.parent = None;
                }
            } else if let Some(parent_index) = entity.parent {
                if parent_index >= total_entities {
                    entity.parent = None;
                }
            }
        }
    }

    fn normalized_clone(&self) -> Self {
        let mut clone = self.clone();
        clone.dependencies.normalize();
        clone.normalize_entities();
        clone
    }

    fn path_wants_binary(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("kscene"))
            .unwrap_or(false)
    }

    fn is_binary_payload(bytes: &[u8]) -> bool {
        bytes.len() >= BINARY_SCENE_MAGIC.len() && &bytes[..BINARY_SCENE_MAGIC.len()] == BINARY_SCENE_MAGIC
    }

    #[cfg(feature = "binary_scene")]
    fn to_binary_bytes(&self) -> Result<Vec<u8>> {
        use lz4_flex::block::compress_prepend_size;
        let payload =
            bincode::serialize(self).with_context(|| "Serializing scene payload before compression")?;
        let mut bytes = Vec::with_capacity(BINARY_SCENE_MAGIC.len() + 4 + payload.len());
        bytes.extend_from_slice(&BINARY_SCENE_MAGIC);
        bytes.extend_from_slice(&BINARY_SCENE_VERSION.to_le_bytes());
        let compressed = compress_prepend_size(&payload);
        bytes.extend_from_slice(&compressed);
        Ok(bytes)
    }

    #[cfg(feature = "binary_scene")]
    fn from_binary_bytes(bytes: &[u8]) -> Result<Self> {
        use lz4_flex::block::decompress_size_prepended;
        if !Self::is_binary_payload(bytes) {
            bail!("buffer does not contain a binary scene payload");
        }
        if bytes.len() < BINARY_SCENE_MAGIC.len() + 4 {
            bail!("binary scene header is truncated");
        }
        let version_offset = BINARY_SCENE_MAGIC.len();
        let mut version_bytes = [0u8; 4];
        version_bytes.copy_from_slice(&bytes[version_offset..version_offset + 4]);
        let version = u32::from_le_bytes(version_bytes);
        if version != BINARY_SCENE_VERSION {
            bail!("unsupported .kscene version {} (expected {})", version, BINARY_SCENE_VERSION);
        }
        let compressed = &bytes[version_offset + 4..];
        let decompressed = decompress_size_prepended(compressed)
            .map_err(|err| anyhow!("Decompressing .kscene payload failed: {err}"))?;
        let scene = bincode::deserialize::<Scene>(&decompressed)
            .map_err(|err| anyhow!("Deserializing .kscene payload failed: {err}"))?;
        Ok(scene)
    }

    pub fn entity_index_by_id(&self, id: &str) -> Option<usize> {
        self.entities.iter().position(|entity| entity.id.as_str() == id)
    }

    pub fn clone_subtree(&self, root_id: &str) -> Option<Vec<SceneEntity>> {
        let root_index = self.entity_index_by_id(root_id)?;
        let mut children_map: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, entity) in self.entities.iter().enumerate() {
            if let Some(parent_id) = entity.parent_id.as_ref() {
                children_map.entry(parent_id.as_str().to_string()).or_default().push(index);
            }
        }

        let mut stack = vec![root_index];
        let mut selected = HashSet::new();
        while let Some(index) = stack.pop() {
            if !selected.insert(index) {
                continue;
            }
            let entity = &self.entities[index];
            if let Some(children) = children_map.get(entity.id.as_str()) {
                stack.extend(children.iter().copied());
            }
        }
        if selected.is_empty() {
            return None;
        }
        let mut ordered = Vec::with_capacity(selected.len());
        for (index, entity) in self.entities.iter().enumerate() {
            if selected.contains(&index) {
                ordered.push(entity.clone());
            }
        }
        Some(ordered)
    }

    pub fn with_fresh_entity_ids(&self) -> Self {
        let mut cloned = self.clone();
        let mut remap: HashMap<String, SceneEntityId> = HashMap::with_capacity(cloned.entities.len());
        for entity in &mut cloned.entities {
            let old_id = entity.id.clone();
            let new_id = SceneEntityId::new();
            remap.insert(old_id.as_str().to_string(), new_id.clone());
            entity.id = new_id;
        }
        for entity in &mut cloned.entities {
            if let Some(existing_parent) = entity.parent_id.clone() {
                if let Some(mapped) = remap.get(existing_parent.as_str()) {
                    entity.parent_id = Some(mapped.clone());
                } else {
                    entity.parent_id = None;
                }
            }
        }
        cloned
    }

    pub fn offset_entities_2d(&mut self, offset: Vec2) {
        if offset.length_squared() == 0.0 {
            return;
        }
        for entity in &mut self.entities {
            let mut translation: Vec2 = entity.transform.translation.clone().into();
            translation += offset;
            entity.transform.translation = translation.into();
            if let Some(transform3d) = entity.transform3d.as_mut() {
                let mut translation3: Vec3 = transform3d.translation.clone().into();
                translation3.x += offset.x;
                translation3.y += offset.y;
                transform3d.translation = translation3.into();
            }
        }
    }

    pub fn offset_entities_3d(&mut self, offset: Vec3) {
        if offset.length_squared() == 0.0 {
            return;
        }
        for entity in &mut self.entities {
            let mut translation: Vec2 = entity.transform.translation.clone().into();
            translation.x += offset.x;
            translation.y += offset.y;
            entity.transform.translation = translation.into();
            if let Some(transform3d) = entity.transform3d.as_mut() {
                let mut translation3: Vec3 = transform3d.translation.clone().into();
                translation3 += offset;
                transform3d.translation = translation3.into();
            }
        }
    }
}

impl TransformData {
    pub fn from_components(translation: glam::Vec2, rotation: f32, scale: glam::Vec2) -> Self {
        Self { translation: translation.into(), rotation, scale: scale.into() }
    }
}

impl Transform3DData {
    pub fn from_components(translation: glam::Vec3, rotation: glam::Quat, scale: glam::Vec3) -> Self {
        Self { translation: translation.into(), rotation: rotation.into(), scale: scale.into() }
    }

    pub fn components(&self) -> (glam::Vec3, glam::Quat, glam::Vec3) {
        (self.translation.clone().into(), self.rotation.clone().into(), self.scale.clone().into())
    }
}

impl From<glam::Vec2> for Vec2Data {
    fn from(value: glam::Vec2) -> Self {
        Self { x: value.x, y: value.y }
    }
}

impl From<Vec2Data> for glam::Vec2 {
    fn from(value: Vec2Data) -> Self {
        glam::Vec2::new(value.x, value.y)
    }
}

impl From<glam::Vec3> for Vec3Data {
    fn from(value: glam::Vec3) -> Self {
        Self { x: value.x, y: value.y, z: value.z }
    }
}

impl From<Vec3Data> for glam::Vec3 {
    fn from(value: Vec3Data) -> Self {
        glam::Vec3::new(value.x, value.y, value.z)
    }
}

impl From<glam::Quat> for QuatData {
    fn from(value: glam::Quat) -> Self {
        let v = value.normalize();
        Self { x: v.x, y: v.y, z: v.z, w: v.w }
    }
}

impl From<QuatData> for glam::Quat {
    fn from(value: QuatData) -> Self {
        glam::Quat::from_xyzw(value.x, value.y, value.z, value.w)
    }
}

impl From<glam::Vec4> for ColorData {
    fn from(value: glam::Vec4) -> Self {
        Self { r: value.x, g: value.y, b: value.z, a: value.w }
    }
}

impl From<ColorData> for glam::Vec4 {
    fn from(value: ColorData) -> Self {
        glam::Vec4::new(value.r, value.g, value.b, value.a)
    }
}
