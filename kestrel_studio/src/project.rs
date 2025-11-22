use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

const RECENT_PROJECTS_PATH: &str = "config/recent_projects.json";
const RECENT_LIMIT: usize = 8;

#[derive(Debug, Clone, Deserialize)]
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
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ProjectConfigPaths {
    pub app: PathBuf,
    pub plugins: PathBuf,
    pub input: PathBuf,
}

impl Default for ProjectManifest {
    fn default() -> Self {
        Self {
            name: None,
            id: None,
            assets: PathBuf::from("assets"),
            config: ProjectConfigPaths::default(),
            startup_scene: PathBuf::from("assets/scenes/quick_save.json"),
            prefabs: PathBuf::from("assets/prefabs"),
            environments: PathBuf::from("assets/environments"),
            scripts_entry: PathBuf::from("assets/scripts/main.rhai"),
            main_atlas: PathBuf::from("assets/images/atlas.json"),
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

/// Resolved project with absolute/normalized paths.
#[derive(Debug, Clone)]
pub struct Project {
    root: PathBuf,
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
}

impl Project {
    /// Load a `.kestrelproj` manifest from disk.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let root = path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
        let contents = fs::read_to_string(path)
            .with_context(|| format!("Failed to read project manifest {}", path.display()))?;
        let manifest: ProjectManifest = serde_json::from_str(&contents)
            .with_context(|| format!("Failed to parse project manifest {}", path.display()))?;
        Self::from_manifest(root, manifest)
    }

    /// Construct a project from defaults rooted at the current directory.
    pub fn default() -> Result<Self> {
        let cwd = std::env::current_dir().context("Failed to read current directory")?;
        Self::from_manifest(cwd, ProjectManifest::default())
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
        let recent = Self::load_recent_list();
        recent.into_iter().next()
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
        assert_eq!(
            project.config_app_path(),
            &dir.path().join("cfg/app.json")
        );
        assert_eq!(
            project.assets_root(),
            &dir.path().join("content")
        );
        assert_eq!(
            project.startup_scene_path(),
            &dir.path().join("content/scenes/start.json")
        );
        assert_eq!(
            project.main_atlas_path(),
            &dir.path().join("content/images/atlas.json")
        );
        assert_eq!(project.name(), Some("My Game"));
    }

    #[test]
    fn falls_back_to_defaults() {
        let project = Project::default().expect("default");
        assert!(project.assets_root().ends_with("assets"));
        assert!(project.config_app_path().ends_with("config/app.json"));
    }
}
