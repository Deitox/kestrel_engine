# Plugin System

Kestrel Engine exposes a lightweight plugin API so tooling or gameplay extensions can hook into the main loop without touching the core crate. There are three main pieces:

1. **`EnginePlugin` trait** – Plugins implement lifecycle hooks (`build`, `update`, `fixed_update`, `on_events`, `shutdown`) plus identity helpers (`name`, `version`, `depends_on`). Each plugin is `'static` and receives a `PluginContext` that exposes vetted entry points into the renderer, ECS, assets, materials/meshes, input, the environment registry, the shared `FeatureRegistry`, and helpers such as `emit_script_message`. Prefer the lightweight facades (`ctx.renderer_api()`, `ctx.assets_api()`, etc.) instead of reaching into the backing types so future engine changes remain non-breaking.
2. **Dynamic loader** – At startup the engine scans `config/plugins.json`, resolves each enabled entry, checks feature requirements, and uses `libloading` to pull an exported factory from compiled `.dll` / `.so` / `.dylib` artifacts. Built-in plugins can also be disabled via the same manifest.
3. **Feature registry** – Strings describe capabilities (`scripts.rhai`, `audio.rodio`, `render.3d`, etc.). Plugins can query the registry (`ctx.features()`) or publish new entries (`ctx.features_mut().register("my.feature")`). The manifest can also gate loading via `requires_features`, while `provides_features` are registered automatically after build.

> Dynamic plugins are compiled in separate Cargo invocations, so Rust `TypeId`s (like Bevy resources) do not line up with the host build. Avoid poking raw ECS resources; rely on the safe helpers exposed on `PluginContext` (`emit_event`, `emit_script_message`, asset/material facades, etc.) so the engine performs the actual mutations on your behalf.

## Manifest format

`config/plugins.json` keeps the dynamic plugin list. Relative `path` values resolve against that file’s directory, and the same manifest can disable built-in plugins so every project has a single source of truth.

```json
{
  "disable_builtins": ["audio"],
  "plugins": [
    {
      "name": "example_dynamic",
      "version": "0.1.0",
      "path": "../plugins/example_dynamic/target/release/example_dynamic.dll",
      "enabled": true,
      "min_engine_api": 1,
      "requires_features": ["scripts.rhai"],
      "provides_features": ["examples.dynamic_overlay"]
    }
  ]
}
```

Fields:

- `disable_builtins`: global array of built-in plugin names to skip (e.g., `"audio"`, `"analytics"`, `"mesh_preview"`). Disabled entries still appear in the Plugin Status panel with the recorded reason.
- `name`: informational label and the key other plugins reference via `depends_on()`.
- `version`: optional string surfaced in the status panel when the manifest disables or fails to load the plugin before the engine can query its real version.
- `path`: full or relative path to the compiled dynamic library. Use the correct extension for your platform (`.dll`, `.so`, `.dylib`).
- `enabled`: optional flag (defaults to `true`) that lets you keep entries in the manifest without loading them.
- `min_engine_api`: optional minimum `ENGINE_PLUGIN_API_VERSION`. Loading fails if the engine exports an older API.
- `requires_features`: optional features that must already be present in the registry.
- `provides_features`: optional list automatically added to the registry after successful registration so other plugins can depend on them.

### Editing the manifest from the editor

- **Dynamic plugins** – The Plugins panel lists every manifest entry with a checkbox matching the `enabled` flag. Toggling a row rewrites `config/plugins.json`, then immediately reloads dynamic libraries so you can test changes without restarting.
- **Built-in plugins** – The same panel exposes checkboxes for the built-in modules listed in `disable_builtins` (`audio`, `analytics`, `mesh_preview`, `scripts`). Updates are saved to the manifest right away and apply on the next engine restart.
- **Out-of-sync entries** – If a toggle targets a plugin that no longer exists in the manifest, the editor posts a warning and reloads the file so the UI reflects the current state.

If the manifest is missing, the loader simply skips dynamic registration.

## Building a plugin

The repo includes `plugins/example_dynamic`, a `cdylib` crate that exercises the API. To rebuild every enabled plugin listed in the manifest, run:

```powershell
pwsh scripts/build_plugins.ps1        # Debug
pwsh scripts/build_plugins.ps1 -Release
```

The script infers each crate’s root from the artifact path and runs `cargo build` with the requested profile.

```shell
cargo build --manifest-path plugins/example_dynamic/Cargo.toml --release
```

On Windows this produces `plugins/example_dynamic/target/release/example_dynamic.dll`. Update `config/plugins.json` with the correct (platform-specific) path, then toggle `enabled` to `true`.

### Debug vs. Release builds

- **Debug (`scripts/build_plugins.ps1`)** – Uses Cargo’s default profile for fast compiles and richer debug info. Artifacts land in `target\debug\` and are best for rapid iteration.
- **Release (`scripts/build_plugins.ps1 -Release`)** – Switches to `cargo build --release`, enabling `opt-level = 3` optimisations defined in `Cargo.toml`. The resulting DLLs (under `target\release\`) are smaller and markedly faster, which is what you’ll ship or benchmark.

Both commands update every enabled entry listed in `config/plugins.json`. If you only care about a single plugin you can still invoke `cargo build --manifest-path plugins/example_dynamic/Cargo.toml [--release]`, but the helper script keeps the manifest and binaries in sync automatically.

The skeleton looks like this:

```rust
use anyhow::Result;
use kestrel_engine::plugins::{
    EnginePlugin, PluginContext, PluginExport, PluginHandle, ENGINE_PLUGIN_API_VERSION,
};

#[derive(Default)]
struct ExamplePlugin;

impl EnginePlugin for ExamplePlugin {
    fn name(&self) -> &'static str { "example" }
    fn version(&self) -> &'static str { "0.1.0" }

    fn build(&mut self, ctx: &mut PluginContext<'_>) -> Result<()> {
        // Publish a feature so other plugins can depend on us.
        ctx.features_mut().register("examples.dynamic_overlay");
        Ok(())
    }

    fn update(&mut self, ctx: &mut PluginContext<'_>, dt: f32) -> Result<()> {
        // Emit a periodic log event without touching ECS internals.
        if dt > 0.0 {
            ctx.emit_script_message(format!(
                "example plugin tick (features visible: {})",
                ctx.features().all().count()
            ));
        }
        Ok(())
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

unsafe extern "C" fn create_plugin() -> PluginHandle {
    let plugin: Box<dyn EnginePlugin> = Box::new(ExamplePlugin::default());
    PluginHandle::from_box(plugin)
}

#[no_mangle]
pub extern "C" fn kestrel_plugin_entry() -> PluginExport {
    PluginExport { api_version: ENGINE_PLUGIN_API_VERSION, create: create_plugin }
}
```

The engine expects every dynamic library to export `kestrel_plugin_entry`, which returns a `PluginExport` describing the targeted API version and a factory function that yields a boxed `EnginePlugin`. As soon as the plugin builds successfully, the loader registers any `provides_features` listed in the manifest along with whatever the plugin publishes during `build()`.

If the loader encounters missing libraries, incompatible API versions, unmet feature requirements, or disabled entries, it logs the failure and records the outcome in the “Plugins” section of the right-hand egui panel so you can see which modules are Loaded / Disabled / Failed without digging through stdout.

## Reloading without restart

After rebuilding a plugin or correcting the manifest, open the in-app Plugins panel and press **Reload plugins**. The engine rescans `config/plugins.json`, updates the status list, and attempts to load any entries that were previously skipped without requiring an application restart.
