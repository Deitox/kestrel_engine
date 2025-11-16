use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PrefabFormat {
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

impl Default for PrefabFormat {
    fn default() -> Self {
        PrefabFormat::Json
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
    revision: u64,
}

impl PrefabLibrary {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), entries: Vec::new(), revision: 0 }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn entries(&self) -> &[PrefabDescriptor] {
        &self.entries
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
}
