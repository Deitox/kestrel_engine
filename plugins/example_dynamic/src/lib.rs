use anyhow::Result;
use kestrel_engine::plugins::{
    EnginePlugin, PluginContext, PluginExport, PluginHandle, ENGINE_PLUGIN_API_VERSION,
};
use std::any::Any;

#[derive(Default)]
struct ExampleDynamicPlugin {
    elapsed: f32,
    fired_events: u32,
}

impl EnginePlugin for ExampleDynamicPlugin {
    fn name(&self) -> &'static str {
        "example_dynamic"
    }

    fn update(&mut self, ctx: &mut PluginContext<'_>, dt: f32) -> Result<()> {
        self.elapsed += dt;
        if self.elapsed > 1.0 {
            self.elapsed = 0.0;
            self.fired_events += 1;
            let message = format!(
                "dynamic plugin heartbeat #{} ({} features visible)",
                self.fired_events,
                ctx.features().all().count()
            );
            ctx.emit_script_message(message);
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
