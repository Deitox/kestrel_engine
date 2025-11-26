use anyhow::{Context, Result};
use serde_json;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum PrefabFormat {
    #[default]
    Json,
    Binary,
}

impl PrefabFormat {
    pub fn extension(self) -> &'static str {
        match self {
            PrefabFormat::Json => "json",
            PrefabFormat::Binary => "kscene",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PrefabFormat::Json => "JSON (.json)",
            PrefabFormat::Binary => "Binary (.kscene)",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            PrefabFormat::Json => "json",
            PrefabFormat::Binary => "kscene",
        }
    }

    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "json" => Some(PrefabFormat::Json),
            "kscene" => Some(PrefabFormat::Binary),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PrefabDescriptor {
    pub name: String,
    pub format: PrefabFormat,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub enum PrefabStatusKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct PrefabStatusMessage {
    pub kind: PrefabStatusKind,
    pub message: String,
}

pub struct PrefabLibrary {
    root: PathBuf,
    entries: Vec<PrefabDescriptor>,
    aliases: HashMap<String, String>,
    revision: u64,
}

impl PrefabLibrary {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), entries: Vec::new(), aliases: HashMap::new(), revision: 0 }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn entries(&self) -> &[PrefabDescriptor] {
        &self.entries
    }

    pub fn aliases(&self) -> &HashMap<String, String> {
        &self.aliases
    }

    pub fn resolve(&self, name: &str) -> Option<PrefabDescriptor> {
        let key = name.trim().to_lowercase();
        if key.is_empty() {
            return None;
        }
        let target = self
            .aliases
            .get(&key)
            .map(|value| value.to_lowercase())
            .unwrap_or_else(|| key.clone());
        let mut json_candidate = None;
        let mut any_candidate = None;
        for entry in &self.entries {
            if entry.name.eq_ignore_ascii_case(&target) {
                if entry.format == PrefabFormat::Json {
                    json_candidate = Some(entry.clone());
                    break;
                } else if any_candidate.is_none() {
                    any_candidate = Some(entry.clone());
                }
            }
        }
        json_candidate.or(any_candidate)
    }

    pub fn ensure_root(&self) -> Result<()> {
        if !self.root.exists() {
            fs::create_dir_all(&self.root)
                .with_context(|| format!("Creating prefab directory {}", self.root.display()))?;
        }
        Ok(())
    }

    pub fn refresh(&mut self) -> Result<()> {
        self.ensure_root()?;
        let mut grouped: BTreeMap<String, PrefabDescriptor> = BTreeMap::new();
        for entry in fs::read_dir(&self.root)
            .with_context(|| format!("Scanning prefabs under {}", self.root.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            if let Some(file_name) = path.file_name().and_then(|name| name.to_str()) {
                if file_name.eq_ignore_ascii_case("aliases.json") {
                    continue;
                }
            }
            let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
                continue;
            };
            let Some(format) = PrefabFormat::from_extension(ext) else {
                continue;
            };
            let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let descriptor = PrefabDescriptor { name: name.to_string(), format, path: path.clone() };
            let key = format!("{}::{}", name.to_lowercase(), format.short_label());
            grouped.insert(key, descriptor);
        }
        self.entries = grouped.into_values().collect();
        self.entries.sort_by(|a, b| {
            a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.format.cmp(&b.format))
        });
        self.aliases = self.load_aliases().unwrap_or_default();
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }

    pub fn path_for(&self, name: &str, format: PrefabFormat) -> PathBuf {
        let mut file_name = name.trim().to_string();
        if file_name.is_empty() {
            file_name.push_str("prefab");
        }
        let sanitized = file_name
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' { ch } else { '_' })
            .collect::<String>();
        self.root.join(format!("{sanitized}.{}", format.extension()))
    }

    pub fn version(&self) -> u64 {
        self.revision
    }

    fn load_aliases(&self) -> Result<HashMap<String, String>> {
        let path = self.root.join("aliases.json");
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let contents =
            fs::read_to_string(&path).with_context(|| format!("Reading prefab alias file {}", path.display()))?;
        let mut aliases: HashMap<String, String> =
            serde_json::from_str(&contents).context("Parsing prefab alias file as JSON object")?;
        let normalized = aliases
            .drain()
            .map(|(alias, target)| (alias.to_lowercase(), target))
            .collect::<HashMap<_, _>>();
        Ok(normalized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn aliases_resolve_and_prefer_json() {
        let dir = tempdir().expect("temp dir");
        let root = dir.path();
        let json_path = root.join("enemy.json");
        let bin_path = root.join("enemy.kscene");
        let aliases_path = root.join("aliases.json");
        std::fs::write(&json_path, "{}").expect("write json prefab");
        std::fs::write(&bin_path, "{}").expect("write binary prefab");
        std::fs::write(&aliases_path, r#"{ "orc": "enemy" }"#).expect("write aliases");

        let mut library = PrefabLibrary::new(root);
        library.refresh().expect("refresh library");

        assert_eq!(library.entries().len(), 2, "should track both prefab formats");
        let resolved = library.resolve("Orc").expect("alias should resolve");
        assert_eq!(resolved.name, "enemy");
        assert_eq!(resolved.format, PrefabFormat::Json, "should prefer json when both exist");
    }
}
