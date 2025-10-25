use crate::assets::AssetManager;
use crate::ecs::EcsWorld;
use crate::environment::EnvironmentRegistry;
use crate::events::GameEvent;
use crate::input::Input;
use crate::material_registry::MaterialRegistry;
use crate::mesh_registry::MeshRegistry;
use crate::renderer::Renderer;
use crate::scripts::ScriptHost;
use crate::time::Time;
use anyhow::Result;
use std::any::Any;

pub struct PluginContext<'a> {
    pub renderer: &'a mut Renderer,
    pub ecs: &'a mut EcsWorld,
    pub assets: &'a mut AssetManager,
    pub input: &'a mut Input,
    pub scripts: &'a mut ScriptHost,
    pub material_registry: &'a mut MaterialRegistry,
    pub mesh_registry: &'a mut MeshRegistry,
    pub environment_registry: &'a mut EnvironmentRegistry,
    pub time: &'a Time,
}

impl<'a> PluginContext<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        renderer: &'a mut Renderer,
        ecs: &'a mut EcsWorld,
        assets: &'a mut AssetManager,
        input: &'a mut Input,
        scripts: &'a mut ScriptHost,
        material_registry: &'a mut MaterialRegistry,
        mesh_registry: &'a mut MeshRegistry,
        environment_registry: &'a mut EnvironmentRegistry,
        time: &'a Time,
    ) -> Self {
        Self {
            renderer,
            ecs,
            assets,
            input,
            scripts,
            material_registry,
            mesh_registry,
            environment_registry,
            time,
        }
    }
}

pub trait EnginePlugin: Any {
    fn name(&self) -> &'static str;

    fn build(&mut self, _ctx: &mut PluginContext<'_>) -> Result<()> {
        Ok(())
    }

    fn update(&mut self, _ctx: &mut PluginContext<'_>, _dt: f32) -> Result<()> {
        Ok(())
    }

    fn fixed_update(&mut self, _ctx: &mut PluginContext<'_>, _dt: f32) -> Result<()> {
        Ok(())
    }

    fn on_events(&mut self, _ctx: &mut PluginContext<'_>, _events: &[GameEvent]) -> Result<()> {
        Ok(())
    }

    fn shutdown(&mut self, _ctx: &mut PluginContext<'_>) -> Result<()> {
        Ok(())
    }

    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

#[derive(Default)]
pub struct PluginManager {
    plugins: Vec<Box<dyn EnginePlugin>>,
}

impl PluginManager {
    pub fn register(&mut self, mut plugin: Box<dyn EnginePlugin>, ctx: &mut PluginContext<'_>) -> Result<()> {
        plugin.build(ctx)?;
        self.plugins.push(plugin);
        Ok(())
    }

    pub fn update(&mut self, ctx: &mut PluginContext<'_>, dt: f32) {
        for plugin in &mut self.plugins {
            let name = plugin.name();
            if let Err(err) = plugin.update(ctx, dt) {
                eprintln!("[plugin:{name}] update failed: {err:?}");
            }
        }
    }

    pub fn fixed_update(&mut self, ctx: &mut PluginContext<'_>, dt: f32) {
        for plugin in &mut self.plugins {
            let name = plugin.name();
            if let Err(err) = plugin.fixed_update(ctx, dt) {
                eprintln!("[plugin:{name}] fixed_update failed: {err:?}");
            }
        }
    }

    pub fn handle_events(&mut self, ctx: &mut PluginContext<'_>, events: &[GameEvent]) {
        if events.is_empty() {
            return;
        }
        for plugin in &mut self.plugins {
            let name = plugin.name();
            if let Err(err) = plugin.on_events(ctx, events) {
                eprintln!("[plugin:{name}] event hook failed: {err:?}");
            }
        }
    }

    pub fn shutdown(&mut self, ctx: &mut PluginContext<'_>) {
        for plugin in &mut self.plugins {
            let name = plugin.name();
            if let Err(err) = plugin.shutdown(ctx) {
                eprintln!("[plugin:{name}] shutdown failed: {err:?}");
            }
        }
    }

    pub fn get<T: EnginePlugin + 'static>(&self) -> Option<&T> {
        self.plugins.iter().find_map(|plugin| plugin.as_any().downcast_ref::<T>())
    }

    pub fn get_mut<T: EnginePlugin + 'static>(&mut self) -> Option<&mut T> {
        self.plugins.iter_mut().find_map(|plugin| plugin.as_any_mut().downcast_mut::<T>())
    }
}
