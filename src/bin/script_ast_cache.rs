use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use kestrel_engine::scripts::ScriptHost;

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let cache_dir = args.get(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("assets/scripts_cache"));
    let script_root = args.get(2).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("assets/scripts"));
    println!(
        "[script-cache] caching scripts under '{}' into '{}'",
        script_root.display(),
        cache_dir.display()
    );
    let scripts = collect_scripts(&script_root)?;
    let mut cached = 0usize;
    for script in scripts {
        let mut host = ScriptHost::new(&script);
        host.set_ast_cache_dir(Some(cache_dir.clone()));
        host.force_reload(None)
            .with_context(|| format!("Compiling '{}'", script.display()))?;
        cached += 1;
    }
    println!("[script-cache] cached {cached} scripts");
    Ok(())
}

fn collect_scripts(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if root.is_file() {
        if root.extension().and_then(|ext| ext.to_str()) == Some("rhai") {
            out.push(root.to_path_buf());
        }
        return Ok(out);
    }
    for entry in std::fs::read_dir(root).with_context(|| format!("Reading '{}'", root.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            out.extend(collect_scripts(&path)?);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rhai") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}
