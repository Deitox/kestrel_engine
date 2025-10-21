use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::mesh::Mesh;
use crate::renderer::{GpuMesh, Renderer};

#[derive(Default)]
pub struct MeshRegistry {
    entries: HashMap<String, MeshEntry>,
    default: String,
}

struct MeshEntry {
    mesh: Mesh,
    gpu: Option<GpuMesh>,
    source: Option<PathBuf>,
}

impl MeshRegistry {
    pub fn new() -> Self {
        let mut registry = MeshRegistry { entries: HashMap::new(), default: String::new() };
        registry.insert_entry("cube", Mesh::cube(1.0), None).expect("cube mesh should insert");
        match Mesh::load_gltf("assets/models/demo_triangle.gltf") {
            Ok(mesh) => {
                let _ = registry.insert_entry(
                    "demo_triangle",
                    mesh,
                    Some(PathBuf::from("assets/models/demo_triangle.gltf")),
                );
                registry.default = "demo_triangle".to_string();
            }
            Err(err) => {
                eprintln!("[mesh] demo_triangle.gltf unavailable: {err:?}");
                registry.default = "cube".to_string();
            }
        }
        if registry.default.is_empty() {
            registry.default = "cube".to_string();
        }
        registry
    }

    fn insert_entry(&mut self, key: impl Into<String>, mesh: Mesh, source: Option<PathBuf>) -> Result<()> {
        let key_str = key.into();
        self.entries.insert(key_str, MeshEntry { mesh, gpu: None, source });
        Ok(())
    }

    pub fn default_key(&self) -> &str {
        &self.default
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(|k| k.as_str())
    }

    pub fn has(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    pub fn ensure_mesh(&mut self, key: &str, path: Option<&str>) -> Result<()> {
        if let Some(entry) = self.entries.get_mut(key) {
            if entry.source.is_none() {
                if let Some(p) = path {
                    entry.source = Some(PathBuf::from(p));
                }
            }
            return Ok(());
        }
        let path = path.ok_or_else(|| anyhow!("Mesh '{key}' not registered and no path provided"))?;
        self.load_from_path(key, path)
    }

    pub fn load_from_path(&mut self, key: &str, path: impl AsRef<Path>) -> Result<()> {
        let path_ref = path.as_ref();
        let mesh = Mesh::load_gltf(path_ref)?;
        self.entries
            .insert(key.to_string(), MeshEntry { mesh, gpu: None, source: Some(path_ref.to_path_buf()) });
        Ok(())
    }

    pub fn ensure_gpu<'a>(&'a mut self, key: &str, renderer: &mut Renderer) -> Result<&'a GpuMesh> {
        let entry =
            self.entries.get_mut(key).ok_or_else(|| anyhow!("Mesh '{key}' not registered in registry"))?;
        if entry.gpu.is_none() {
            let gpu = renderer.create_gpu_mesh(&entry.mesh)?;
            entry.gpu = Some(gpu);
        }
        Ok(entry.gpu.as_ref().expect("GPU mesh populated"))
    }

    pub fn mesh_source(&self, key: &str) -> Option<&Path> {
        self.entries.get(key).and_then(|entry| entry.source.as_deref())
    }

    pub fn gpu_mesh(&self, key: &str) -> Option<&GpuMesh> {
        self.entries.get(key).and_then(|entry| entry.gpu.as_ref())
    }
}
