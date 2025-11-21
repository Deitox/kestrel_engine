use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::material_registry::MaterialRegistry;
use crate::mesh::{Mesh, MeshBounds, MeshSubset};
use crate::renderer::{GpuMesh, Renderer};

pub struct MeshRegistry {
    entries: HashMap<String, MeshEntry>,
    default: String,
    revision: u64,
}

struct MeshEntry {
    mesh: Mesh,
    gpu: Option<GpuMesh>,
    source: Option<PathBuf>,
    ref_count: usize,
    permanent: bool,
    material_keys: Vec<String>,
}

impl MeshRegistry {
    pub fn new(materials: &mut MaterialRegistry) -> Self {
        let mut registry = MeshRegistry { entries: HashMap::new(), default: String::new(), revision: 0 };
        registry
            .insert_entry("cube", Mesh::cube(1.0), None, Vec::new(), true)
            .expect("cube mesh should insert");
        match crate::mesh::Mesh::load_gltf_with_materials("assets/models/demo_triangle.gltf") {
            Ok(import) => {
                let material_keys: Vec<String> = import.materials.iter().map(|mat| mat.key.clone()).collect();
                materials.register_gltf_import(&import.materials, &import.textures);
                let mesh = import.mesh;
                let _ = registry.insert_entry(
                    "demo_triangle",
                    mesh,
                    Some(PathBuf::from("assets/models/demo_triangle.gltf")),
                    material_keys,
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
        material_keys: Vec<String>,
        permanent: bool,
    ) -> Result<()> {
        let key_str = key.into();
        if self.entries.contains_key(&key_str) {
            return Err(anyhow!("Mesh '{key_str}' already registered in registry"));
        }
        self.entries.insert(
            key_str,
            MeshEntry { mesh, gpu: None, source, ref_count: 0, permanent, material_keys },
        );
        self.bump_revision();
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

    pub fn ensure_mesh(
        &mut self,
        key: &str,
        path: Option<&str>,
        materials: &mut MaterialRegistry,
    ) -> Result<()> {
        if let Some(entry) = self.entries.get_mut(key) {
            if entry.source.is_none() {
                if let Some(p) = path {
                    entry.source = Some(PathBuf::from(p));
                    self.bump_revision();
                }
            }
            return Ok(());
        }
        let path = path.ok_or_else(|| anyhow!("Mesh '{key}' not registered and no path provided"))?;
        self.load_from_path(key, path, materials)
    }

    pub fn load_from_path(
        &mut self,
        key: &str,
        path: impl AsRef<Path>,
        materials: &mut MaterialRegistry,
    ) -> Result<()> {
        if self.entries.contains_key(key) {
            return Err(anyhow!("Mesh '{key}' already registered in registry"));
        }
        let path_ref = path.as_ref();
        let import = Mesh::load_gltf_with_materials(path_ref)?;
        materials.register_gltf_import(&import.materials, &import.textures);
        let mesh = import.mesh;
        let material_keys: Vec<String> = import.materials.iter().map(|mat| mat.key.clone()).collect();
        let mut retained: Vec<String> = Vec::new();
        for mat_key in &material_keys {
            if let Err(err) = materials.retain(mat_key) {
                for retained_key in retained {
                    materials.release(&retained_key);
                }
                return Err(err);
            }
            retained.push(mat_key.clone());
        }
        if let Err(err) =
            self.insert_entry(key.to_string(), mesh, Some(path_ref.to_path_buf()), material_keys, false)
        {
            for mat_key in retained {
                materials.release(&mat_key);
            }
            return Err(err);
        }
        Ok(())
    }

    pub fn retain_mesh(
        &mut self,
        key: &str,
        path: Option<&str>,
        materials: &mut MaterialRegistry,
    ) -> Result<()> {
        self.ensure_mesh(key, path, materials)?;
        if let Some(entry) = self.entries.get_mut(key) {
            entry.ref_count = entry.ref_count.saturating_add(1);
        }
        Ok(())
    }

    pub fn release_mesh(&mut self, key: &str, materials: &mut MaterialRegistry) {
        let mut remove = false;
        let mut material_keys: Vec<String> = Vec::new();
        if let Some(entry) = self.entries.get_mut(key) {
            if entry.ref_count > 0 {
                entry.ref_count -= 1;
            }
            if entry.ref_count == 0 && !entry.permanent {
                remove = true;
                material_keys = entry.material_keys.clone();
            }
        }
        if remove {
            self.entries.remove(key);
            for mat_key in material_keys {
                materials.release(&mat_key);
            }
            self.bump_revision();
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

    pub fn version(&self) -> u64 {
        self.revision
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material_registry::MaterialRegistry;

    #[test]
    fn retain_release_tracks_counts() {
        let mut materials = MaterialRegistry::new();
        let mut registry = MeshRegistry::new(&mut materials);
        assert_eq!(registry.mesh_ref_count("cube"), Some(0));
        registry.retain_mesh("cube", None, &mut materials).expect("retain cube");
        assert_eq!(registry.mesh_ref_count("cube"), Some(1));
        registry.release_mesh("cube", &mut materials);
        assert_eq!(registry.mesh_ref_count("cube"), Some(0));

        registry
            .load_from_path("temp_triangle", "assets/models/demo_triangle.gltf", &mut materials)
            .expect("load temp mesh");
        registry
            .retain_mesh("temp_triangle", Some("assets/models/demo_triangle.gltf"), &mut materials)
            .expect("retain temp mesh");
        assert_eq!(registry.mesh_ref_count("temp_triangle"), Some(1));
        registry.release_mesh("temp_triangle", &mut materials);
        assert!(!registry.has("temp_triangle"), "non-permanent mesh should be removed at refcount 0");
    }

    #[test]
    fn ensure_mesh_source_updates_revision() {
        let mut materials = MaterialRegistry::new();
        let mut registry = MeshRegistry::new(&mut materials);
        let before = registry.version();
        registry.ensure_mesh("cube", Some("assets/models/cube.gltf"), &mut materials).unwrap();
        assert!(registry.version() > before, "revision should bump when source is recorded");
        let recorded = registry.mesh_source("cube").expect("cube should have a source set");
        assert!(recorded.ends_with("assets/models/cube.gltf"));
    }

    #[test]
    fn duplicate_mesh_key_is_rejected() {
        let mut materials = MaterialRegistry::new();
        let mut registry = MeshRegistry::new(&mut materials);
        registry
            .load_from_path("temp_triangle", "assets/models/demo_triangle.gltf", &mut materials)
            .expect("first load ok");
        let err = registry
            .load_from_path("temp_triangle", "assets/models/demo_triangle.gltf", &mut materials)
            .expect_err("duplicate load should fail");
        let message = err.to_string();
        assert!(message.contains("already registered"), "unexpected error: {message}");
    }

    #[test]
    fn release_without_retain_cleans_mesh_and_materials() {
        let mut materials = MaterialRegistry::new();
        let mut registry = MeshRegistry::new(&mut materials);
        registry
            .load_from_path("temp_triangle", "assets/models/demo_triangle.gltf", &mut materials)
            .expect("load temp mesh");
        let subset_materials: Vec<String> = registry
            .mesh_subsets("temp_triangle")
            .unwrap()
            .iter()
            .filter_map(|subset| subset.material.clone())
            .collect();
        assert!(!subset_materials.is_empty(), "import should register materials");
        for key in &subset_materials {
            assert!(materials.has(key), "material '{key}' should exist after import");
        }

        registry.release_mesh("temp_triangle", &mut materials);
        assert!(!registry.has("temp_triangle"), "mesh should be removed even without prior retain");
        for key in subset_materials {
            assert!(
                !materials.has(&key),
                "material '{key}' should be released when mesh is dropped without references"
            );
        }
    }

    #[test]
    fn releasing_mesh_cleans_imported_materials() {
        let mut materials = MaterialRegistry::new();
        let mut registry = MeshRegistry::new(&mut materials);
        registry
            .load_from_path("temp_triangle", "assets/models/demo_triangle.gltf", &mut materials)
            .expect("load temp mesh");
        let subset_materials: Vec<String> = registry
            .mesh_subsets("temp_triangle")
            .unwrap()
            .iter()
            .filter_map(|subset| subset.material.clone())
            .collect();
        assert!(!subset_materials.is_empty(), "import should register materials");
        for key in &subset_materials {
            assert!(materials.has(key), "material '{key}' should exist after import");
        }
        registry
            .retain_mesh("temp_triangle", Some("assets/models/demo_triangle.gltf"), &mut materials)
            .expect("retain temp mesh");
        registry.release_mesh("temp_triangle", &mut materials);
        for key in subset_materials {
            assert!(!materials.has(&key), "material '{key}' should be released with mesh");
        }
        assert!(!registry.has("temp_triangle"));
    }
}
