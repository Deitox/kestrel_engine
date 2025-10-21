use crate::assets::AssetManager;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scene {
    #[serde(default)]
    pub dependencies: SceneDependencies,
    #[serde(default)]
    pub entities: Vec<SceneEntity>,
}

impl Default for Scene {
    fn default() -> Self {
        Self { dependencies: SceneDependencies::default(), entities: Vec::new() }
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneDependencies {
    #[serde(default)]
    atlases: Vec<AtlasDependencyRepr>,
    #[serde(default)]
    meshes: Vec<MeshDependencyRepr>,
}

impl SceneDependencies {
    pub fn from_entities<F>(entities: &[SceneEntity], assets: &AssetManager, mesh_source: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
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
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneEntity {
    #[serde(default)]
    pub name: Option<String>,
    pub transform: TransformData,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sprite: Option<SpriteData>,
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
    pub parent: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformData {
    pub translation: Vec2Data,
    pub rotation: f32,
    pub scale: Vec2Data,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpriteData {
    pub atlas: String,
    pub region: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshData {
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vec2Data {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorData {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
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
        let mut scene = serde_json::from_slice::<Scene>(&bytes)
            .with_context(|| format!("Parsing scene file {}", path.display()))?;
        scene.dependencies.normalize();
        Ok(scene)
    }

    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Creating scene directory {}", parent.display()))?;
        }
        let mut normalized = self.clone();
        normalized.dependencies.normalize();
        let json = serde_json::to_string_pretty(&normalized)?;
        fs::write(path, json.as_bytes()).with_context(|| format!("Writing scene file {}", path.display()))?;
        Ok(())
    }
}

impl TransformData {
    pub fn from_components(translation: glam::Vec2, rotation: f32, scale: glam::Vec2) -> Self {
        Self { translation: translation.into(), rotation, scale: scale.into() }
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
