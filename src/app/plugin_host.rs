use crate::plugins::{
    apply_manifest_builtin_toggles, apply_manifest_dynamic_toggles, EnginePlugin, ManifestBuiltinToggle,
    ManifestBuiltinToggleOutcome, ManifestDynamicToggle, ManifestDynamicToggleOutcome, PluginContext,
    PluginManager, PluginManifest,
};
use anyhow::{anyhow, Result};
use std::collections::HashSet;
use std::path::PathBuf;

pub(crate) struct BuiltinPluginFactory {
    pub(crate) name: &'static str,
    builder: Box<dyn Fn() -> Box<dyn EnginePlugin>>,
}

impl BuiltinPluginFactory {
    pub(crate) fn new<F>(name: &'static str, builder: F) -> Self
    where
        F: Fn() -> Box<dyn EnginePlugin> + 'static,
    {
        Self { name, builder: Box::new(builder) }
    }

    fn build(&self) -> Box<dyn EnginePlugin> {
        (self.builder)()
    }
}

#[derive(Debug)]
pub(crate) struct PluginToggleSummary {
    pub(crate) dynamic: ManifestDynamicToggleOutcome,
    pub(crate) builtin: ManifestBuiltinToggleOutcome,
}

impl PluginToggleSummary {
    pub(crate) fn changed(&self) -> bool {
        self.dynamic.changed || self.builtin.changed
    }
}

pub(crate) struct PluginHost {
    manifest: Option<PluginManifest>,
    manifest_path: PathBuf,
    manifest_error: Option<String>,
}

impl PluginHost {
    pub(crate) fn new(manifest_path: impl Into<PathBuf>) -> Self {
        let manifest_path = manifest_path.into();
        let (manifest, manifest_error) = match PluginManager::load_manifest(&manifest_path) {
            Ok(data) => (data, None),
            Err(err) => {
                let message = format!("failed to parse manifest '{}': {err:?}", manifest_path.display());
                eprintln!("[plugin] {message}");
                (None, Some(message))
            }
        };
        Self { manifest, manifest_path, manifest_error }
    }

    pub(crate) fn manifest(&self) -> Option<&PluginManifest> {
        self.manifest.as_ref()
    }

    pub(crate) fn manifest_error(&self) -> Option<&str> {
        self.manifest_error.as_deref()
    }

    pub(crate) fn register_builtins(
        &mut self,
        manager: &mut PluginManager,
        ctx: &mut PluginContext<'_>,
        factories: &[BuiltinPluginFactory],
    ) {
        let disabled = self.disabled_builtins();
        for factory in factories {
            if disabled.contains(factory.name) {
                manager.record_builtin_disabled(factory.name, "disabled via config/plugins.json");
                continue;
            }
            if let Err(err) = manager.register(factory.build(), ctx) {
                eprintln!("[plugin] failed to register {} plugin: {err:?}", factory.name);
            }
        }
        if let Some(manifest) = self.manifest.as_ref() {
            match manager.load_dynamic_from_manifest(manifest, ctx) {
                Ok(loaded) => {
                    if !loaded.is_empty() {
                        println!("[plugin] loaded dynamic plugins: {}", loaded.join(", "));
                    }
                }
                Err(err) => eprintln!("[plugin] failed to load dynamic plugins: {err:?}"),
            }
        }
    }

    pub(crate) fn reload_dynamic_from_disk(
        &mut self,
        manager: &mut PluginManager,
        ctx: &mut PluginContext<'_>,
    ) -> Result<Vec<String>> {
        self.reload_manifest_from_disk()?;
        self.load_dynamic(manager, ctx)
    }

    pub(crate) fn load_dynamic(
        &mut self,
        manager: &mut PluginManager,
        ctx: &mut PluginContext<'_>,
    ) -> Result<Vec<String>> {
        let manifest = self.manifest.clone().ok_or_else(|| anyhow!("Plugin manifest not found"))?;
        manager.unload_dynamic_plugins(ctx);
        manager.clear_dynamic_statuses();
        manager.load_dynamic_from_manifest(&manifest, ctx)
    }

    pub(crate) fn apply_manifest_toggles(
        &mut self,
        dynamic: &[ManifestDynamicToggle],
        builtin: &[ManifestBuiltinToggle],
    ) -> Result<PluginToggleSummary> {
        let manifest = self.manifest.as_mut().ok_or_else(|| anyhow!("Plugin manifest not found"))?;
        let dynamic_outcome = apply_manifest_dynamic_toggles(manifest, dynamic);
        let builtin_outcome = apply_manifest_builtin_toggles(manifest, builtin);
        if dynamic_outcome.changed || builtin_outcome.changed {
            manifest.save()?;
        }
        Ok(PluginToggleSummary { dynamic: dynamic_outcome, builtin: builtin_outcome })
    }

    pub(crate) fn reload_manifest_from_disk(&mut self) -> Result<()> {
        match PluginManager::load_manifest(&self.manifest_path) {
            Ok(manifest) => {
                self.manifest = manifest;
                self.manifest_error = None;
                Ok(())
            }
            Err(err) => {
                self.manifest = None;
                self.manifest_error =
                    Some(format!("failed to load manifest '{}': {err:?}", self.manifest_path.display()));
                Err(err)
            }
        }
    }

    fn disabled_builtins(&self) -> HashSet<String> {
        self.manifest
            .as_ref()
            .map(|manifest| manifest.disabled_builtins().map(|name| name.to_string()).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn toggle_application_persists_manifest() {
        let dir = tempdir().expect("temp dir");
        let manifest_path = dir.path().join("plugins.json");
        fs::write(
            &manifest_path,
            r#"
{
  "disable_builtins": ["audio"],
  "plugins": [
    { "name": "alpha", "path": "alpha.dll", "enabled": true }
  ]
}
"#,
        )
        .expect("manifest written");
        let mut host = PluginHost::new(&manifest_path);
        let summary = host
            .apply_manifest_toggles(
                &[ManifestDynamicToggle { name: "alpha".to_string(), new_enabled: false }],
                &[ManifestBuiltinToggle { name: "analytics".to_string(), disable: true }],
            )
            .expect("toggles applied");
        assert!(summary.dynamic.changed, "dynamic entry should change");
        assert!(summary.builtin.changed, "builtin entry should change");
        let reloaded =
            PluginManager::load_manifest(&manifest_path).expect("reload succeeds").expect("manifest present");
        let alpha =
            reloaded.entries().iter().find(|entry| entry.name == "alpha").expect("alpha entry present");
        assert!(!alpha.enabled, "alpha disabled after toggle");
        assert!(reloaded.is_builtin_disabled("analytics"), "analytics disabled");
    }

    #[test]
    fn apply_toggles_handles_missing_manifest() {
        let mut host = PluginHost::new("missing.json");
        host.manifest = None;
        let err = host.apply_manifest_toggles(&[], &[]).expect_err("missing manifest errors");
        assert!(err.to_string().contains("manifest"), "error mentions manifest");
    }

    #[test]
    fn manifest_errors_reported_and_cleared() {
        let dir = tempdir().expect("temp dir");
        let manifest_path = dir.path().join("plugins.json");
        fs::write(&manifest_path, "{ invalid json").expect("write invalid manifest");
        let mut host = PluginHost::new(&manifest_path);
        assert!(host.manifest().is_none(), "invalid manifest should not load");
        let captured = host.manifest_error().expect("error recorded");
        assert!(captured.contains("failed to parse manifest"), "captured error explains parse failure");

        fs::write(
            &manifest_path,
            r#"{
  "disable_builtins": [],
  "plugins": []
}"#,
        )
        .expect("valid manifest written");
        host.reload_manifest_from_disk().expect("reload succeeds");
        assert!(host.manifest().is_some(), "valid manifest loads after reload");
        assert!(host.manifest_error().is_none(), "error cleared after successful load");
    }
}
