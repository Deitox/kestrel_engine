use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::mesh::{Mesh, MeshBounds, MeshSubset};
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
    ref_count: usize,
    permanent: bool,
}

impl MeshRegistry {
    pub fn new() -> Self {
        let mut registry = MeshRegistry { entries: HashMap::new(), default: String::new() };
        registry.insert_entry("cube", Mesh::cube(1.0), None, true).expect("cube mesh should insert");
        match Mesh::load_gltf("assets/models/demo_triangle.gltf") {
            Ok(mesh) => {
                let _ = registry.insert_entry(
                    "demo_triangle",
                    mesh,
                    Some(PathBuf::from("assets/models/demo_triangle.gltf")),
                    true,
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

    fn insert_entry(
        &mut self,
        key: impl Into<String>,
        mesh: Mesh,
        source: Option<PathBuf>,
        permanent: bool,
    ) -> Result<()> {
        let key_str = key.into();
        self.entries.insert(key_str, MeshEntry { mesh, gpu: None, source, ref_count: 0, permanent });
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
        self.insert_entry(key.to_string(), mesh, Some(path_ref.to_path_buf()), false)
    }

    pub fn retain_mesh(&mut self, key: &str, path: Option<&str>) -> Result<()> {
        self.ensure_mesh(key, path)?;
        if let Some(entry) = self.entries.get_mut(key) {
            entry.ref_count = entry.ref_count.saturating_add(1);
        }
        Ok(())
    }

    pub fn release_mesh(&mut self, key: &str) {
        let mut remove = false;
        if let Some(entry) = self.entries.get_mut(key) {
            if entry.ref_count == 0 {
                return;
            }
            entry.ref_count -= 1;
            if entry.ref_count == 0 && !entry.permanent {
                remove = true;
            }
        }
        if remove {
            self.entries.remove(key);
        }
    }

    pub fn mesh_ref_count(&self, key: &str) -> Option<usize> {
        self.entries.get(key).map(|entry| entry.ref_count)
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

    pub fn mesh(&self, key: &str) -> Option<&Mesh> {
        self.entries.get(key).map(|entry| &entry.mesh)
    }

    pub fn mesh_subsets(&self, key: &str) -> Option<&[MeshSubset]> {
        self.entries.get(key).map(|entry| entry.mesh.subsets.as_slice())
    }

    pub fn mesh_bounds(&self, key: &str) -> Option<&MeshBounds> {
        self.entries.get(key).map(|entry| &entry.mesh.bounds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retain_release_tracks_counts() {
        let mut registry = MeshRegistry::new();
        assert_eq!(registry.mesh_ref_count("cube"), Some(0));
        registry.retain_mesh("cube", None).expect("retain cube");
        assert_eq!(registry.mesh_ref_count("cube"), Some(1));
        registry.release_mesh("cube");
        assert_eq!(registry.mesh_ref_count("cube"), Some(0));

        registry.load_from_path("temp_triangle", "assets/models/demo_triangle.gltf").expect("load temp mesh");
        registry
            .retain_mesh("temp_triangle", Some("assets/models/demo_triangle.gltf"))
            .expect("retain temp mesh");
        assert_eq!(registry.mesh_ref_count("temp_triangle"), Some(1));
        registry.release_mesh("temp_triangle");
        assert!(!registry.has("temp_triangle"), "non-permanent mesh should be removed at refcount 0");
    }
}
