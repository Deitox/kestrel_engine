use anyhow::Result;
use kestrel_engine::plugins::{
    EnginePlugin, PluginContext, PluginExport, PluginHandle, ENGINE_PLUGIN_API_VERSION,
};
use std::{any::Any, time::Duration};

#[derive(Default)]
struct ExampleDynamicPlugin {
    elapsed: f32,
    fired_events: u32,
    watchdog_sleep_ms: Option<u64>,
    watchdog_armed: bool,
    force_renderer_violation: bool,
}

impl EnginePlugin for ExampleDynamicPlugin {
    fn name(&self) -> &'static str {
        "example_dynamic"
    }

    fn version(&self) -> &'static str {
        "0.1.0"
    }

    fn build(&mut self, ctx: &mut PluginContext<'_>) -> Result<()> {
        ctx.assets_mut()?.load_atlas("main", "assets/images/atlas.json")?;
        if let Ok(value) = std::env::var("EXAMPLE_DYNAMIC_SLEEP_MS") {
            if let Ok(parsed) = value.parse::<u64>() {
                self.watchdog_sleep_ms = Some(parsed);
            }
        }
        if let Ok(value) = std::env::var("EXAMPLE_DYNAMIC_FORCE_RENDERER_VIOLATION") {
            self.force_renderer_violation =
                value == "1" || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("yes");
        }
        Ok(())
    }

    fn update(&mut self, ctx: &mut PluginContext<'_>, dt: f32) -> Result<()> {
        if !self.watchdog_armed {
            if let Some(ms) = self.watchdog_sleep_ms.take() {
                self.watchdog_armed = true;
                std::thread::sleep(Duration::from_millis(ms));
            }
        }
        if self.force_renderer_violation {
            let _ = ctx.renderer_mut();
        }
        self.elapsed += dt;
        if self.elapsed > 1.0 {
            self.elapsed = 0.0;
            self.fired_events += 1;
            let message = format!(
                "dynamic plugin heartbeat #{} ({} features visible)",
                self.fired_events,
                ctx.features().all().count()
            );
            ctx.emit_script_message(message)?;
        }
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

unsafe extern "C" fn create_plugin() -> PluginHandle {
    let plugin: Box<dyn EnginePlugin> = Box::new(ExampleDynamicPlugin::default());
    PluginHandle::from_box(plugin)
}

#[no_mangle]
pub extern "C" fn kestrel_plugin_entry() -> PluginExport {
    PluginExport { api_version: ENGINE_PLUGIN_API_VERSION, create: create_plugin }
}
