# Plugin System

Kestrel Engine exposes a lightweight plugin API so tooling or gameplay extensions can hook into the main loop without touching the core crate. There are three moving pieces:

1. **`EnginePlugin` trait** – Plugins implement lifecycle hooks (`build`, `update`, `fixed_update`, `on_events`, `shutdown`) plus `name` and downcasting helpers. Each hook receives a `PluginContext` granting access to renderer, ECS, input, assets, material/mesh registries, the environment registry, and the shared `FeatureRegistry`.
2. **Dynamic loader** – At startup the engine scans `config/plugins.json`, resolves each enabled entry, checks feature requirements, and uses `libloading` to pull an exported factory from the compiled dynamic library (`.dll` / `.so` / `.dylib`). Libraries stay resident for the lifetime of the plugin.
3. **Feature registry** – Strings describe capabilities (`scripts.rhai`, `audio.rodio`, `render.3d`, etc.). Plugins can read the registry (`ctx.features()`) to branch on available systems or publish new features via `ctx.features_mut().register("my.feature")`. The manifest can enforce prerequisites via `requires_features`, and the manager automatically records `provides_features` after registration.

> ⚠️ Dynamic plugins are compiled in a separate Cargo invocation, so Rust type IDs (like Bevy resources) do not line up with the host build. Avoid calling directly into `ctx.ecs` or other low-level types. Instead, rely on the helper methods exposed on `PluginContext` (`emit_event`, `emit_script_message`, forthcoming bridges) and plain data (strings, numbers) so the engine performs the actual mutations on your behalf.

## Manifest format

`config/plugins.json` keeps the dynamic plugin list. Relative `path` values resolve against that file's directory.

```json
{
  "plugins": [
    {
      "name": "example_dynamic",
      "path": "plugins/example_dynamic/target/debug/example_dynamic.dll",
      "enabled": true,
      "min_engine_api": 1,
      "requires_features": ["scripts.rhai"],
      "provides_features": ["examples.dynamic_overlay"]
    }
  ]
}
```

Fields:

- `name`: purely informational, used in log output.
- `path`: full or relative path to the compiled dynamic library. Use the correct extension for your platform (`.dll`, `.so`, `.dylib`).
- `enabled`: optional flag (defaults to `true`) that lets you keep entries in the manifest without loading them.
- `min_engine_api`: optional minimum `ENGINE_PLUGIN_API_VERSION`. Loading fails if the engine exports an older API.
- `requires_features`: optional features that must already exist in the registry. If any are missing, the entry is skipped.
- `provides_features`: optional list automatically added to the registry after successful registration so other plugins can depend on them.

If the manifest is missing, the loader simply skips dynamic registration, so you can remove the file entirely on builds that don't ship plugins.

## Building a plugin

The repo includes `plugins/example_dynamic`, a `cdylib` crate that exercises the API:

```shell
cargo build --manifest-path plugins/example_dynamic/Cargo.toml --release
```

On Windows this produces `plugins/example_dynamic/target/release/example_dynamic.dll`. Update `config/plugins.json` with the correct path (and extension) for your platform, then toggle `enabled` to `true`.

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

    fn build(&mut self, ctx: &mut PluginContext<'_>) -> Result<()> {
        // Publish a feature so other plugins can depend on us.
        ctx.features_mut().register("examples.dynamic_overlay");
        Ok(())
    }

    fn update(&mut self, ctx: &mut PluginContext<'_>, dt: f32) -> Result<()> {
        // Emit a periodic log event without touching ECS internals.
        if dt > 0.0 {
            ctx.emit_script_message(format!(
                \"example plugin tick (features visible: {})\",
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

If the loader encounters missing libraries, incompatible API versions, or unmet feature requirements it logs the failure and moves on so one plugin cannot block the entire startup sequence.
