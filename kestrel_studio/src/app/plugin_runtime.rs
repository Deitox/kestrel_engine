use crate::app::plugin_host::PluginHost;
use crate::assets::AssetManager;
use crate::ecs::EcsWorld;
use crate::environment::EnvironmentRegistry;
use crate::events::GameEvent;
use crate::input::Input;
use crate::material_registry::MaterialRegistry;
use crate::mesh_registry::MeshRegistry;
use crate::plugins::{PluginContext, PluginManager};
use crate::renderer::Renderer;
use crate::time::Time;
use bevy_ecs::entity::Entity;

pub(crate) struct PluginRuntime {
    host: PluginHost,
    manager: PluginManager,
}

pub(crate) struct PluginContextInputs<'a> {
    pub renderer: &'a mut Renderer,
    pub ecs: &'a mut EcsWorld,
    pub assets: &'a mut AssetManager,
    pub input: &'a mut Input,
    pub material_registry: &'a mut MaterialRegistry,
    pub mesh_registry: &'a mut MeshRegistry,
    pub environment_registry: &'a mut EnvironmentRegistry,
    pub time: &'a Time,
    pub event_emitter: fn(&mut EcsWorld, GameEvent),
    pub selected_entity: Option<Entity>,
}

impl PluginRuntime {
    pub(crate) fn new(host: PluginHost, manager: PluginManager) -> Self {
        Self { host, manager }
    }

    pub(crate) fn host(&self) -> &PluginHost {
        &self.host
    }

    pub(crate) fn host_mut(&mut self) -> &mut PluginHost {
        &mut self.host
    }

    pub(crate) fn manager(&self) -> &PluginManager {
        &self.manager
    }

    pub(crate) fn manager_mut(&mut self) -> &mut PluginManager {
        &mut self.manager
    }

    pub(crate) fn with_context<F, R>(&mut self, inputs: PluginContextInputs<'_>, f: F) -> R
    where
        F: FnOnce(&mut PluginHost, &mut PluginManager, &mut PluginContext<'_>) -> R,
    {
        let feature_handle = self.manager.feature_handle();
        let capability_handle = self.manager.capability_tracker_handle();
        let mut ctx = PluginContext::new(
            inputs.renderer,
            inputs.ecs,
            inputs.assets,
            inputs.input,
            inputs.material_registry,
            inputs.mesh_registry,
            inputs.environment_registry,
            inputs.time,
            inputs.event_emitter,
            feature_handle,
            inputs.selected_entity,
            capability_handle,
        );
        let result = f(&mut self.host, &mut self.manager, &mut ctx);
        drop(ctx);
        result
    }
}
