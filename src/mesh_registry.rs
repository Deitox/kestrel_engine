use blake3::Hasher as Blake3Hasher;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{anyhow, Result};

use crate::config::MeshHashAlgorithm;
use crate::material_registry::MaterialRegistry;
use crate::mesh::{Mesh, MeshBounds, MeshSubset};
use crate::renderer::{GpuMesh, Renderer};

pub struct MeshRegistry {
    entries: HashMap<String, MeshEntry>,
    default: String,
    revision: u64,
    fingerprint_cache: HashMap<PathBuf, CachedFingerprint>,
    fingerprint_cache_order: VecDeque<PathBuf>,
    hash_algorithm: MeshHashAlgorithm,
    fingerprint_cache_limit: usize,
}

struct MeshEntry {
    mesh: Mesh,
    gpu: Option<GpuMesh>,
    source: Option<PathBuf>,
    fingerprint: Option<u128>,
    ref_count: usize,
    permanent: bool,
    material_keys: Vec<String>,
}

struct CachedFingerprint {
    len: u64,
    modified: Option<u128>,
    hash: u128,
    sample: Option<u64>,
    algorithm: MeshHashAlgorithm,
}

impl MeshRegistry {
    pub fn new(materials: &mut MaterialRegistry) -> Self {
        Self::new_with_hash(materials, MeshHashAlgorithm::default(), None)
    }

    pub fn new_with_hash(
        materials: &mut MaterialRegistry,
        hash_algorithm: MeshHashAlgorithm,
        cache_limit: Option<usize>,
    ) -> Self {
        let mut registry = MeshRegistry {
            entries: HashMap::new(),
            default: String::new(),
            revision: 0,
            fingerprint_cache: HashMap::new(),
            fingerprint_cache_order: VecDeque::new(),
            hash_algorithm,
            fingerprint_cache_limit: cache_limit.unwrap_or(512),
        };
        registry
            .insert_entry("cube", Mesh::cube(1.0), None, None, Vec::new(), true)
            .expect("cube mesh should insert");
        match crate::mesh::Mesh::load_gltf_with_materials("assets/models/demo_triangle.gltf") {
            Ok(import) => {
                let material_keys: Vec<String> = import.materials.iter().map(|mat| mat.key.clone()).collect();
                materials.register_gltf_import(&import.materials, &import.textures);
                let mesh = import.mesh;
                let source = PathBuf::from("assets/models/demo_triangle.gltf");
                let fingerprint = registry.mesh_source_fingerprint(&source);
                let mut retained: Vec<String> = Vec::new();
                let default_result: Result<()> = (|| {
                    for mat_key in &material_keys {
                        materials.retain(mat_key)?;
                        retained.push(mat_key.clone());
                    }
                    registry.insert_entry(
                        "demo_triangle",
                        mesh,
                        Some(source),
                        fingerprint,
                        material_keys,
                        true,
                    )
                })();
                match default_result {
                    Ok(()) => {
                        registry.default = "demo_triangle".to_string();
                    }
                    Err(err) => {
                        eprintln!("[mesh] failed to load default demo_triangle.gltf: {err:?}");
                        for mat_key in retained {
                            materials.release(&mat_key);
                        }
                    }
                }
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
        fingerprint: Option<u128>,
        material_keys: Vec<String>,
        permanent: bool,
    ) -> Result<()> {
        let key_str = key.into();
        if self.entries.contains_key(&key_str) {
            return Err(anyhow!("Mesh '{key_str}' already registered in registry"));
        }
        self.entries.insert(
            key_str,
            MeshEntry { mesh, gpu: None, source, fingerprint, ref_count: 0, permanent, material_keys },
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
        if let Some(entry) = self.entries.get(key) {
            let recorded_source = entry.source.clone();
            let recorded_fingerprint = entry.fingerprint;
            if let Some(p) = path {
                let supplied = PathBuf::from(p);
                let supplied_fingerprint = self.mesh_source_fingerprint(&supplied);
                let needs_reload = match recorded_source {
                    Some(existing) => existing != supplied || recorded_fingerprint != supplied_fingerprint,
                    None => true,
                };
                if needs_reload {
                    return self.reload_from_path(key, &supplied, materials);
                }
            } else if let Some(existing) = recorded_source {
                let latest_fingerprint = self.mesh_source_fingerprint(&existing);
                if recorded_fingerprint != latest_fingerprint {
                    return self.reload_from_path(key, &existing, materials);
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
        let snapshot = materials.register_gltf_import_with_snapshot(&import.materials, &import.textures);
        let mesh = import.mesh;
        let material_keys: Vec<String> = import.materials.iter().map(|mat| mat.key.clone()).collect();
        let mut retained: Vec<String> = Vec::new();
        for mat_key in &material_keys {
            if let Err(err) = materials.retain(mat_key) {
                for retained_key in retained {
                    materials.release(&retained_key);
                }
                snapshot.rollback(materials);
                return Err(err);
            }
            retained.push(mat_key.clone());
        }
        let fingerprint = self.mesh_source_fingerprint(path_ref);
        if let Err(err) = self.insert_entry(
            key.to_string(),
            mesh,
            Some(path_ref.to_path_buf()),
            fingerprint,
            material_keys,
            false,
        )
        {
            for mat_key in retained {
                materials.release(&mat_key);
            }
            snapshot.rollback(materials);
            return Err(err);
        }
        Ok(())
    }

    fn reload_from_path(
        &mut self,
        key: &str,
        path: &Path,
        materials: &mut MaterialRegistry,
    ) -> Result<()> {
        let (ref_count, permanent, old_materials) = {
            let entry =
                self.entries.get(key).ok_or_else(|| anyhow!("Mesh '{key}' not registered for reload"))?;
            (entry.ref_count, entry.permanent, entry.material_keys.clone())
        };

        let import = Mesh::load_gltf_with_materials(path)?;
        let snapshot = materials.register_gltf_import_with_snapshot(&import.materials, &import.textures);
        let material_keys: Vec<String> = import.materials.iter().map(|mat| mat.key.clone()).collect();
        let mut retained: Vec<String> = Vec::new();
        for mat_key in &material_keys {
            if let Err(err) = materials.retain(mat_key) {
                for retained_key in retained {
                    materials.release(&retained_key);
                }
                snapshot.rollback(materials);
                return Err(err);
            }
            retained.push(mat_key.clone());
        }

        let fingerprint = self.mesh_source_fingerprint(path);

        if let Some(entry) = self.entries.get_mut(key) {
            entry.mesh = import.mesh;
            entry.gpu = None;
            entry.source = Some(path.to_path_buf());
            entry.fingerprint = fingerprint;
            entry.material_keys = material_keys;
            entry.ref_count = ref_count;
            entry.permanent = permanent;
        }

        for mat_key in old_materials {
            materials.release(&mat_key);
        }

        self.bump_revision();
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

    pub fn fingerprint_for_path(&mut self, path: &Path) -> Option<u128> {
        self.mesh_source_fingerprint(path)
    }

    fn refresh_cache_entry(&mut self, path: &Path) {
        if let Some(pos) = self.fingerprint_cache_order.iter().position(|p| p == path) {
            self.fingerprint_cache_order.remove(pos);
        }
        self.fingerprint_cache_order.push_back(path.to_path_buf());
    }

    fn insert_fingerprint(
        &mut self,
        path: &Path,
        len: u64,
        modified: Option<u128>,
        fingerprint: FingerprintResult,
    ) {
        let path_buf = path.to_path_buf();
        self.refresh_cache_entry(path);
        self.fingerprint_cache.insert(
            path_buf,
            CachedFingerprint {
                len,
                modified,
                hash: fingerprint.hash,
                sample: fingerprint.sample,
                algorithm: self.hash_algorithm,
            },
        );
        while self.fingerprint_cache_order.len() > self.fingerprint_cache_limit {
            if let Some(evicted) = self.fingerprint_cache_order.pop_front() {
                self.fingerprint_cache.remove(&evicted);
            }
        }
    }

    fn mesh_source_fingerprint(&mut self, path: &Path) -> Option<u128> {
        let metadata = fs::metadata(path).ok()?;
        let len = metadata.len();
        let modified =
            metadata.modified().ok().and_then(|ts| ts.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos());

        match self.hash_algorithm {
            MeshHashAlgorithm::Blake3 => {
                // Always read the file to avoid missing metadata-stable changes.
                let computed = hash_file_with_blake3(path)?;
                let hash = computed.hash;
                self.insert_fingerprint(path, len, modified, computed);
                Some(hash)
            }
            MeshHashAlgorithm::Metadata => {
                let sample = quick_sample_hash(path);
                if let Some((cached_sample, cached_hash)) = self
                    .fingerprint_cache
                    .get(path)
                    .filter(|entry| {
                        entry.len == len
                            && entry.modified == modified
                            && entry.algorithm == self.hash_algorithm
                    })
                    .map(|entry| (entry.sample, entry.hash))
                {
                    if samples_match(cached_sample, sample) {
                        self.refresh_cache_entry(path);
                        return Some(cached_hash);
                    }
                }

                let computed = FingerprintResult {
                    hash: metadata_fingerprint(len, modified, sample),
                    sample,
                };
                let hash = computed.hash;
                self.insert_fingerprint(path, len, modified, computed);
                Some(hash)
            }
        }
    }
}

const FINGERPRINT_SAMPLE_BYTES: usize = 4_096;

struct FingerprintResult {
    hash: u128,
    sample: Option<u64>,
}

fn hash_file_with_blake3(path: &Path) -> Option<FingerprintResult> {
    let mut file = fs::File::open(path).ok()?;
    let mut hasher = Blake3Hasher::new();
    let mut buf = [0u8; 131_072];

    loop {
        let read = file.read(&mut buf).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }

    let finalized = hasher.finalize();
    let mut out = [0u8; 16];
    out.copy_from_slice(&finalized.as_bytes()[..16]);

    let sample = quick_sample_hash(path);

    Some(FingerprintResult { hash: u128::from_le_bytes(out), sample })
}

fn metadata_fingerprint(len: u64, modified: Option<u128>, sample: Option<u64>) -> u128 {
    let mut hasher = DefaultHasher::new();
    len.hash(&mut hasher);
    modified.hash(&mut hasher);
    sample.hash(&mut hasher);
    hasher.finish() as u128
}

fn quick_sample_hash(path: &Path) -> Option<u64> {
    let metadata = fs::metadata(path).ok()?;
    let file_len = metadata.len();
    let mut file = fs::File::open(path).ok()?;
    let mut buf = [0u8; FINGERPRINT_SAMPLE_BYTES];
    let mut hasher = DefaultHasher::new();

    // Head
    let head_read = file.read(&mut buf).ok()?;
    if head_read == 0 {
        return None;
    }
    hasher.write(&buf[..head_read]);

    // Middle sample if the file is large enough to avoid overlapping head/tail.
    if file_len > (FINGERPRINT_SAMPLE_BYTES as u64 * 3) {
        let mid_start = file_len / 2;
        let mid_offset = mid_start.saturating_sub((FINGERPRINT_SAMPLE_BYTES as u64) / 2);
        file.seek(SeekFrom::Start(mid_offset)).ok()?;
        let mid_read = file.read(&mut buf).ok()?;
        if mid_read > 0 {
            hasher.write(&buf[..mid_read]);
        }
    }

    // Tail
    if file_len > FINGERPRINT_SAMPLE_BYTES as u64 {
        let tail_start = file_len.saturating_sub(FINGERPRINT_SAMPLE_BYTES as u64);
        if tail_start > 0 {
            file.seek(SeekFrom::Start(tail_start)).ok()?;
            let tail_read = file.read(&mut buf).ok()?;
            if tail_read > 0 {
                hasher.write(&buf[..tail_read]);
            }
        }
    } else {
        let tail_read = file.read(&mut buf).ok()?;
        if tail_read > 0 {
            hasher.write(&buf[..tail_read]);
        }
    }

    Some(hasher.finish())
}

fn samples_match(expected: Option<u64>, actual: Option<u64>) -> bool {
    match (expected, actual) {
        (Some(lhs), Some(rhs)) => lhs == rhs,
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material_registry::MaterialRegistry;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::NamedTempFile;

    const SIMPLE_TRIANGLE_GLTF: &str = r#"{
  "asset": { "version": "2.0" },
  "buffers": [
    {
      "uri": "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAIA/AAAAAAAAAAAAAIA/AAAAAAAAAAAAAIA/AAAAAAAAAAAAAIA/AAAAAAAAAAAAAIA/AAAAAAEAAAACAAAA",
      "byteLength": 108
    }
  ],
  "bufferViews": [
    { "buffer": 0, "byteOffset": 0, "byteLength": 36, "target": 34962 },
    { "buffer": 0, "byteOffset": 36, "byteLength": 36, "target": 34962 },
    { "buffer": 0, "byteOffset": 72, "byteLength": 24, "target": 34962 },
    { "buffer": 0, "byteOffset": 96, "byteLength": 12, "target": 34963 }
  ],
  "accessors": [
    { "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0] },
    { "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 1], "max": [0, 0, 1] },
    { "bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC2", "min": [0, 0], "max": [1, 1] },
    { "bufferView": 3, "componentType": 5125, "count": 3, "type": "SCALAR", "min": [0], "max": [2] }
  ],
  "materials": [
    {
      "name": "MATERIAL_NAME",
      "pbrMetallicRoughness": { "baseColorFactor": [1, 1, 1, 1] }
    }
  ],
  "meshes": [
    {
      "name": "Tri",
      "primitives": [
        {
          "attributes": { "POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2 },
          "indices": 3,
          "material": 0
        }
      ]
    }
  ],
  "nodes": [
    { "mesh": 0, "name": "Root", "translation": [0, 0, 0] }
  ],
  "scenes": [
    { "nodes": [0] }
  ],
  "scene": 0
}"#;

    fn write_gltf(path: &Path, material_name: &str) {
        let json = SIMPLE_TRIANGLE_GLTF.replace("MATERIAL_NAME", material_name);
        std::fs::write(path, json.as_bytes()).expect("write gltf json");
    }

    fn write_temp_gltf(material_name: &str) -> NamedTempFile {
        let file = NamedTempFile::new().expect("temp gltf file");
        write_gltf(file.path(), material_name);
        file
    }

    #[test]
    fn retain_release_tracks_counts() {
        let mut materials = MaterialRegistry::new();
        let mut registry = MeshRegistry::new(&mut materials);
        assert_eq!(registry.mesh_ref_count("cube"), Some(0));
        registry.retain_mesh("cube", None, &mut materials).expect("retain cube");
        assert_eq!(registry.mesh_ref_count("cube"), Some(1));
        registry.release_mesh("cube", &mut materials);
        assert_eq!(registry.mesh_ref_count("cube"), Some(0));

        let gltf = write_temp_gltf("MatCount");
        registry
            .load_from_path("temp_triangle", gltf.path(), &mut materials)
            .expect("load temp mesh");
        registry
            .retain_mesh("temp_triangle", gltf.path().to_str(), &mut materials)
            .expect("retain temp mesh");
        assert_eq!(registry.mesh_ref_count("temp_triangle"), Some(1));
        registry.release_mesh("temp_triangle", &mut materials);
        assert!(!registry.has("temp_triangle"), "non-permanent mesh should be removed at refcount 0");
    }

    #[test]
    fn ensure_mesh_source_updates_revision() {
        let mut materials = MaterialRegistry::new();
        let mut registry = MeshRegistry::new(&mut materials);
        let gltf = write_temp_gltf("MatRevision");
        let gltf_path_str = gltf.path().to_str();
        let before = registry.version();
        registry.ensure_mesh("cube", gltf_path_str, &mut materials).unwrap();
        assert!(registry.version() > before, "revision should bump when source is recorded");
        let recorded = registry.mesh_source("cube").expect("cube should have a source set");
        assert_eq!(recorded, gltf.path());
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
        let gltf = write_temp_gltf("MatRelease");
        registry.load_from_path("temp_triangle", gltf.path(), &mut materials).expect("load temp mesh");
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
        let gltf = write_temp_gltf("MatReleaseRetained");
        registry.load_from_path("temp_triangle", gltf.path(), &mut materials).expect("load temp mesh");
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
            .retain_mesh("temp_triangle", gltf.path().to_str(), &mut materials)
            .expect("retain temp mesh");
        registry.release_mesh("temp_triangle", &mut materials);
        for key in subset_materials {
            assert!(!materials.has(&key), "material '{key}' should be released with mesh");
        }
        assert!(!registry.has("temp_triangle"));
    }

    #[test]
    fn ensure_mesh_reloads_when_source_changes() {
        let mut materials = MaterialRegistry::new();
        let mut registry = MeshRegistry::new(&mut materials);

        let gltf_a = write_temp_gltf("MatA");
        let gltf_b = write_temp_gltf("MatB");
        let key = "temp_reload";

        registry
            .load_from_path(key, gltf_a.path(), &mut materials)
            .expect("first load should succeed");
        let mat_a_key = format!("{}::MatA", gltf_a.path().display());
        assert!(materials.has(&mat_a_key));
        let before = registry.version();

        registry
            .ensure_mesh(key, gltf_b.path().to_str(), &mut materials)
            .expect("reload should succeed");
        let mat_b_key = format!("{}::MatB", gltf_b.path().display());
        assert!(materials.has(&mat_b_key), "new material should be registered");
        assert!(!materials.has(&mat_a_key), "old material should be released after reload");

        let recorded_source = registry.mesh_source(key).expect("source should be recorded");
        assert_eq!(recorded_source, gltf_b.path(), "new source should replace old source");
        assert!(registry.version() > before, "revision should advance when reloads happen");
    }

    #[test]
    fn ensure_mesh_reloads_when_source_contents_change() {
        let mut materials = MaterialRegistry::new();
        let mut registry = MeshRegistry::new(&mut materials);

        let gltf = write_temp_gltf("MatOriginal");
        let key = "temp_reload_same_path";

        registry
            .load_from_path(key, gltf.path(), &mut materials)
            .expect("first load should succeed");
        let original_mat_key = format!("{}::MatOriginal", gltf.path().display());
        assert!(materials.has(&original_mat_key));
        let before = registry.version();

        std::thread::sleep(Duration::from_millis(5));
        write_gltf(gltf.path(), "MatReloadedLong");

        registry
            .ensure_mesh(key, gltf.path().to_str(), &mut materials)
            .expect("reload should succeed for in-place edits");
        let reloaded_mat_key = format!("{}::MatReloadedLong", gltf.path().display());
        assert!(materials.has(&reloaded_mat_key), "new material should be registered");
        assert!(
            !materials.has(&original_mat_key),
            "old material should be released after same-path reload"
        );

        let recorded_source = registry.mesh_source(key).expect("source should stay recorded");
        assert_eq!(recorded_source, gltf.path(), "source path should remain the same");
        assert!(registry.version() > before, "revision should advance when content reloads happen");
    }

    #[test]
    fn ensure_mesh_reloads_in_place_without_explicit_path() {
        let mut materials = MaterialRegistry::new();
        let mut registry = MeshRegistry::new(&mut materials);

        let gltf = write_temp_gltf("MatAlpha");
        let key = "temp_reload_no_path";

        registry.load_from_path(key, gltf.path(), &mut materials).expect("first load should succeed");
        let mat_alpha = format!("{}::MatAlpha", gltf.path().display());
        assert!(materials.has(&mat_alpha));
        let before = registry.version();

        std::thread::sleep(Duration::from_millis(5));
        write_gltf(gltf.path(), "MatBeta");

        registry.ensure_mesh(key, None, &mut materials).expect("reload should work without path");
        let mat_beta = format!("{}::MatBeta", gltf.path().display());
        assert!(materials.has(&mat_beta), "new material should be registered");
        assert!(
            !materials.has(&mat_alpha),
            "old material should be released after same-path reload"
        );
        assert!(registry.version() > before, "revision should advance when content reloads happen");
    }

    #[test]
    fn blake3_rehashes_even_when_metadata_matches() {
        let mut materials = MaterialRegistry::new();
        let mut registry = MeshRegistry::new(&mut materials);

        let gltf = write_temp_gltf("MatHash");
        let expected = hash_file_with_blake3(gltf.path()).expect("hash should compute");

        let metadata = std::fs::metadata(gltf.path()).expect("metadata should exist");
        let modified = metadata
            .modified()
            .ok()
            .and_then(|ts| ts.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos());

        let stale_hash = expected.hash ^ 0xFFFF;
        registry.fingerprint_cache.insert(
            gltf.path().to_path_buf(),
            CachedFingerprint {
                len: metadata.len(),
                modified,
                hash: stale_hash,
                sample: expected.sample.map(|s| s ^ 0x1234_5678),
                algorithm: MeshHashAlgorithm::Blake3,
            },
        );
        registry.fingerprint_cache_order.push_back(gltf.path().to_path_buf());

        let actual = registry.mesh_source_fingerprint(gltf.path()).expect("hash should recompute");
        assert_eq!(
            actual, expected.hash,
            "Blake3 hashing should read the file even when metadata matches cached entry"
        );
    }

    #[test]
    fn blake3_rehashes_even_when_metadata_and_sample_match() {
        let mut materials = MaterialRegistry::new();
        let mut registry = MeshRegistry::new(&mut materials);

        let gltf = write_temp_gltf("MatHashSample");
        let expected = hash_file_with_blake3(gltf.path()).expect("hash should compute");

        let metadata = std::fs::metadata(gltf.path()).expect("metadata should exist");
        let modified = metadata
            .modified()
            .ok()
            .and_then(|ts| ts.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos());

        // Pretend the cached hash is stale but the quick sample still matches.
        let stale_hash = expected.hash ^ 0xABCD;
        registry.fingerprint_cache.insert(
            gltf.path().to_path_buf(),
            CachedFingerprint {
                len: metadata.len(),
                modified,
                hash: stale_hash,
                sample: expected.sample,
                algorithm: MeshHashAlgorithm::Blake3,
            },
        );
        registry.fingerprint_cache_order.push_back(gltf.path().to_path_buf());

        let actual = registry.mesh_source_fingerprint(gltf.path()).expect("hash should recompute");
        assert_eq!(
            actual, expected.hash,
            "Blake3 hashing should recompute even when metadata and sample match"
        );
    }

    #[test]
    fn metadata_fingerprint_changes_when_sample_differs() {
        let len = 1_024;
        let modified = Some(123_456u128);
        let sample_a = metadata_fingerprint(len, modified, Some(0xABCD_1234u64));
        let sample_b = metadata_fingerprint(len, modified, Some(0xFFFF_0001u64));
        assert_ne!(sample_a, sample_b, "sample should influence metadata fingerprint");
        let missing_sample = metadata_fingerprint(len, modified, None);
        assert_ne!(sample_a, missing_sample, "absent sample should change fingerprint");
    }
}
