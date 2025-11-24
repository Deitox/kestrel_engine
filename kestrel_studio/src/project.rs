use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const RECENT_PROJECTS_PATH: &str = "config/recent_projects.json";
const DEFAULT_MANIFEST_NAME: &str = "project.kestrelproj";
const RECENT_LIMIT: usize = 8;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ProjectManifest {
    pub name: Option<String>,
    pub id: Option<String>,
    pub assets: PathBuf,
    pub config: ProjectConfigPaths,
    pub startup_scene: PathBuf,
    pub prefabs: PathBuf,
    pub environments: PathBuf,
    pub scripts_entry: PathBuf,
    pub main_atlas: PathBuf,
    pub plugins: Vec<ProjectPluginDescriptor>,
    pub build: ProjectBuildSettings,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ProjectConfigPaths {
    pub app: PathBuf,
    pub plugins: PathBuf,
    pub input: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ProjectPluginDescriptor {
    pub name: String,
    pub path: PathBuf,
    pub enabled: bool,
    pub capabilities: Vec<crate::plugins::PluginCapability>,
    pub trust: crate::plugins::PluginTrust,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ProjectBuildSettings {
    pub targets: Vec<ProjectBuildTarget>,
    pub default_target: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ProjectBuildTarget {
    pub name: String,
    pub triple: String,
    pub profile: BuildProfile,
    pub output: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildProfile {
    Debug,
    Release,
}

impl Default for ProjectManifest {
    fn default() -> Self {
        Self {
            name: None,
            id: None,
            assets: PathBuf::from("assets"),
            config: ProjectConfigPaths::default(),
            startup_scene: PathBuf::from("assets/scenes/blank.json"),
            prefabs: PathBuf::from("assets/prefabs"),
            environments: PathBuf::from("assets/environments"),
            scripts_entry: PathBuf::from("assets/scripts/main.rhai"),
            main_atlas: PathBuf::from("assets/images/atlas.json"),
            plugins: Vec::new(),
            build: ProjectBuildSettings::default(),
        }
    }
}

impl Default for ProjectConfigPaths {
    fn default() -> Self {
        Self {
            app: PathBuf::from("config/app.json"),
            plugins: PathBuf::from("config/plugins.json"),
            input: PathBuf::from("config/input.json"),
        }
    }
}

impl Default for ProjectPluginDescriptor {
    fn default() -> Self {
        Self {
            name: String::new(),
            path: PathBuf::new(),
            enabled: true,
            capabilities: vec![crate::plugins::PluginCapability::All],
            trust: crate::plugins::PluginTrust::Full,
        }
    }
}

impl Default for ProjectBuildSettings {
    fn default() -> Self {
        let windows = ProjectBuildTarget::default();
        let linux = ProjectBuildTarget {
            name: "linux-x64-release".to_string(),
            triple: "x86_64-unknown-linux-gnu".to_string(),
            profile: BuildProfile::Release,
            output: PathBuf::from("build/release"),
        };
        Self { targets: vec![windows, linux], default_target: Some("windows-x64-debug".to_string()) }
    }
}

impl Default for ProjectBuildTarget {
    fn default() -> Self {
        Self {
            name: "windows-x64-debug".to_string(),
            triple: "x86_64-pc-windows-msvc".to_string(),
            profile: BuildProfile::Debug,
            output: PathBuf::from("build/debug"),
        }
    }
}

impl Default for BuildProfile {
    fn default() -> Self {
        BuildProfile::Debug
    }
}

/// Resolved project with absolute/normalized paths.
#[derive(Debug, Clone)]
pub struct Project {
    root: PathBuf,
    manifest_path: Option<PathBuf>,
    manifest: ProjectManifest,
    assets_root: PathBuf,
    config_app: PathBuf,
    config_plugins: PathBuf,
    config_input: PathBuf,
    startup_scene: PathBuf,
    prefabs: PathBuf,
    environments: PathBuf,
    scripts_entry: PathBuf,
    main_atlas: PathBuf,
    plugins: Vec<ProjectPluginDescriptor>,
    build: ProjectBuildSettings,
}

impl Project {
    /// Load a `.kestrelproj` manifest from disk.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let manifest_path = Self::resolve_manifest_path(path.as_ref());
        let root = manifest_path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
        let contents = fs::read_to_string(&manifest_path)
            .with_context(|| format!("Failed to read project manifest {}", manifest_path.display()))?;
        let manifest: ProjectManifest = serde_json::from_str(&contents)
            .with_context(|| format!("Failed to parse project manifest {}", manifest_path.display()))?;
        let mut project = Self::from_manifest(root, manifest)?;
        project.manifest_path = Some(manifest_path);
        Ok(project)
    }

    /// Construct a project from defaults rooted at the current directory.
    pub fn default() -> Result<Self> {
        let cwd = std::env::current_dir().context("Failed to read current directory")?;
        Self::from_manifest(cwd, ProjectManifest::default())
    }

    /// Create a new project rooted at `path`, seeding it with default configs/assets if present.
    pub fn create_new(path: impl AsRef<Path>, name: Option<String>) -> Result<Self> {
        let root = path.as_ref();
        if root.exists() {
            let mut entries = fs::read_dir(root)
                .with_context(|| format!("Failed to inspect project directory {}", root.display()))?;
            if entries.next().is_some() {
                return Err(anyhow!(
                    "Project directory '{}' is not empty; choose an empty path or remove existing files first.",
                    root.display()
                ));
            }
        }
        fs::create_dir_all(root)
            .with_context(|| format!("Failed to create project dir {}", root.display()))?;
        let assets_src = Path::new("assets");
        let config_src = Path::new("config");
        let prefabs_src = Path::new("plugins");
        if assets_src.exists() {
            if let Err(err) = copy_dir_recursive(assets_src, &root.join("assets")) {
                eprintln!("[project] failed to copy default assets: {err:?}");
            }
        }
        if config_src.exists() {
            if let Err(err) = copy_dir_recursive(config_src, &root.join("config")) {
                eprintln!("[project] failed to copy default config: {err:?}");
            }
        }
        if prefabs_src.exists() {
            // Plugins are optional; best-effort copy so manifests can resolve sample dynamic plugins.
            if let Err(err) = copy_dir_recursive(prefabs_src, &root.join("plugins")) {
                eprintln!("[project] failed to copy plugin templates: {err:?}");
            }
        }
        let manifest_path = root.join(DEFAULT_MANIFEST_NAME);
        let mut manifest = ProjectManifest::default();
        if let Some(name) = name {
            manifest.id = Some(normalize_id(&name));
            manifest.name = Some(name);
        }
        manifest.assets = PathBuf::from("assets");
        manifest.config = ProjectConfigPaths::default();
        manifest.startup_scene = PathBuf::from("assets/scenes/blank.json");
        manifest.prefabs = PathBuf::from("assets/prefabs");
        manifest.environments = PathBuf::from("assets/environments");
        manifest.scripts_entry = PathBuf::from("assets/scripts/main.rhai");
        manifest.main_atlas = PathBuf::from("assets/images/atlas.json");
        Self::save_manifest(&manifest, &manifest_path)?;
        let mut project = Self::from_manifest(root.to_path_buf(), manifest)?;
        project.manifest_path = Some(manifest_path);
        Ok(project)
    }

    /// Persist a manifest to disk.
    pub fn save_manifest(manifest: &ProjectManifest, path: impl AsRef<Path>) -> Result<()> {
        let json = serde_json::to_string_pretty(manifest)?;
        fs::write(path.as_ref(), format!("{json}\n"))
            .with_context(|| format!("Failed to write project manifest {}", path.as_ref().display()))
    }

    /// Resolve a manifest path from a provided path. Directories will look for `project.kestrelproj`.
    fn resolve_manifest_path(path: &Path) -> PathBuf {
        if path.is_dir() {
            return path.join(DEFAULT_MANIFEST_NAME);
        }
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or_default();
        if ext.is_empty() && path.is_file() {
            return path.to_path_buf();
        }
        if ext.is_empty() && path.exists() {
            // Provided a path to an existing directory without trailing slash.
            return path.join(DEFAULT_MANIFEST_NAME);
        }
        path.to_path_buf()
    }

    fn from_manifest(root: PathBuf, manifest: ProjectManifest) -> Result<Self> {
        let resolve = |p: &Path| {
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                root.join(p)
            }
        };
        let project = Self {
            assets_root: resolve(&manifest.assets),
            config_app: resolve(&manifest.config.app),
            config_plugins: resolve(&manifest.config.plugins),
            config_input: resolve(&manifest.config.input),
            startup_scene: resolve(&manifest.startup_scene),
            prefabs: resolve(&manifest.prefabs),
            environments: resolve(&manifest.environments),
            scripts_entry: resolve(&manifest.scripts_entry),
            main_atlas: resolve(&manifest.main_atlas),
            root,
            manifest_path: None,
            plugins: manifest.plugins.clone(),
            build: manifest.build.clone(),
            manifest,
        };
        Ok(project)
    }

    pub fn name(&self) -> Option<&str> {
        self.manifest.name.as_deref()
    }

    pub fn id(&self) -> Option<&str> {
        self.manifest.id.as_deref()
    }

    pub fn assets_root(&self) -> &Path {
        &self.assets_root
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest_path(&self) -> Option<&Path> {
        self.manifest_path.as_deref()
    }

    pub fn manifest_path_or_default(&self) -> PathBuf {
        self.manifest_path.clone().unwrap_or_else(|| self.root.join(DEFAULT_MANIFEST_NAME))
    }

    pub fn config_app_path(&self) -> &Path {
        &self.config_app
    }

    pub fn config_plugins_path(&self) -> &Path {
        &self.config_plugins
    }

    pub fn config_input_path(&self) -> &Path {
        &self.config_input
    }

    pub fn startup_scene_path(&self) -> &Path {
        &self.startup_scene
    }

    pub fn prefab_root(&self) -> &Path {
        &self.prefabs
    }

    pub fn environments_root(&self) -> &Path {
        &self.environments
    }

    pub fn scripts_entry_path(&self) -> &Path {
        &self.scripts_entry
    }

    pub fn main_atlas_path(&self) -> &Path {
        &self.main_atlas
    }

    pub fn plugins(&self) -> &[ProjectPluginDescriptor] {
        &self.plugins
    }

    pub fn build(&self) -> &ProjectBuildSettings {
        &self.build
    }

    pub fn join_assets(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.assets_root.join(relative)
    }

    pub fn display_path(path: &Path) -> String {
        path.display().to_string()
    }

    pub fn describe(&self) -> String {
        let name = self.name().map(|n| n.to_string()).unwrap_or_else(|| "<unnamed>".to_string());
        let id = self.id().map(|i| format!(" ({i})")).unwrap_or_default();
        format!("{name}{id} @ {}", self.root.display())
    }

    /// Load the most recently opened project path, if any.
    pub fn load_recent() -> Option<PathBuf> {
        Self::recent_projects().into_iter().next()
    }

    pub fn recent_projects() -> Vec<PathBuf> {
        Self::load_recent_list()
    }

    /// Update the recent project list, deduping and truncating.
    pub fn record_recent(path: &Path) {
        let mut recent = Self::load_recent_list();
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        recent.retain(|p| p != &canonical);
        recent.insert(0, canonical);
        if recent.len() > RECENT_LIMIT {
            recent.truncate(RECENT_LIMIT);
        }
        if let Err(err) = Self::store_recent_list(&recent) {
            eprintln!("[project] failed to persist recent projects: {err}");
        }
    }

    fn load_recent_list() -> Vec<PathBuf> {
        let path = Path::new(RECENT_PROJECTS_PATH);
        if !path.exists() {
            return Vec::new();
        }
        let data = match fs::read_to_string(path) {
            Ok(data) => data,
            Err(err) => {
                eprintln!("[project] failed to read recent list: {err}");
                return Vec::new();
            }
        };
        match serde_json::from_str::<Vec<String>>(&data) {
            Ok(list) => list.into_iter().map(PathBuf::from).collect(),
            Err(err) => {
                eprintln!("[project] failed to parse recent list: {err}");
                Vec::new()
            }
        }
    }

    fn store_recent_list(paths: &[PathBuf]) -> Result<()> {
        let path = Path::new(RECENT_PROJECTS_PATH);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create recent projects dir {}", parent.display()))?;
        }
        let data = serde_json::to_string_pretty(
            &paths.iter().map(|p| Project::display_path(p.as_path())).collect::<Vec<_>>(),
        )?;
        fs::write(path, data)
            .with_context(|| format!("Failed to write recent projects list {}", path.display()))?;
        Ok(())
    }
}

fn normalize_id(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect()
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("Failed to create {}", dst.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("Failed to read {}", src.display()))? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if ty.is_file() {
            fs::copy(&src_path, &dst_path).with_context(|| {
                format!("Failed to copy {} to {}", src_path.display(), dst_path.display())
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn resolves_relative_paths_against_manifest_dir() {
        let dir = tempdir().expect("tempdir");
        let manifest_path = dir.path().join("MyGame.kestrelproj");
        let manifest = r#"{
            "name": "My Game",
            "assets": "content",
            "config": { "app": "cfg/app.json", "plugins": "cfg/plugins.json", "input": "cfg/input.json" },
            "startup_scene": "content/scenes/start.json",
            "prefabs": "content/prefabs",
            "environments": "content/env",
            "scripts_entry": "content/scripts/main.rhai",
            "main_atlas": "content/images/atlas.json"
        }"#;
        let mut file = fs::File::create(&manifest_path).expect("manifest");
        file.write_all(manifest.as_bytes()).expect("write");

        let project = Project::load(&manifest_path).expect("load project");
        assert!(project.config_app_path().is_absolute());
        assert_eq!(project.config_app_path(), &dir.path().join("cfg/app.json"));
        assert_eq!(project.assets_root(), &dir.path().join("content"));
        assert_eq!(project.startup_scene_path(), &dir.path().join("content/scenes/start.json"));
        assert_eq!(project.main_atlas_path(), &dir.path().join("content/images/atlas.json"));
        assert_eq!(project.name(), Some("My Game"));
    }

    #[test]
    fn falls_back_to_defaults() {
        let project = Project::default().expect("default");
        assert!(project.assets_root().ends_with("assets"));
        assert!(project.config_app_path().ends_with("config/app.json"));
    }
}
