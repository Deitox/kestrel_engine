use crate::ecs::ParticleBudgetMetrics;
use crate::events::GameEvent;
use crate::plugins::{EnginePlugin, PluginContext};
use anyhow::Result;
use std::any::Any;
use std::collections::VecDeque;

pub struct AnalyticsPlugin {
    frame_hist: Vec<f32>,
    frame_capacity: usize,
    events: VecDeque<GameEvent>,
    event_capacity: usize,
    particle_budget: Option<ParticleBudgetMetrics>,
}

impl AnalyticsPlugin {
    pub fn new(frame_capacity: usize, event_capacity: usize) -> Self {
        Self {
            frame_hist: Vec::with_capacity(frame_capacity.min(1_024)),
            frame_capacity: frame_capacity.max(1),
            events: VecDeque::with_capacity(event_capacity.min(1_024)),
            event_capacity: event_capacity.max(1),
            particle_budget: None,
        }
    }

    pub fn frame_plot_points(&self) -> Vec<[f64; 2]> {
        self.frame_hist.iter().enumerate().map(|(i, v)| [i as f64, *v as f64]).collect()
    }

    pub fn recent_events(&self) -> impl Iterator<Item = &GameEvent> {
        self.events.iter()
    }

    pub fn clear_frame_history(&mut self) {
        self.frame_hist.clear();
    }

    pub fn record_particle_budget(&mut self, metrics: ParticleBudgetMetrics) {
        self.particle_budget = Some(metrics);
    }

    pub fn particle_budget(&self) -> Option<ParticleBudgetMetrics> {
        self.particle_budget
    }
}

impl Default for AnalyticsPlugin {
    fn default() -> Self {
        Self::new(240, 32)
    }
}

impl EnginePlugin for AnalyticsPlugin {
    fn name(&self) -> &'static str {
        "analytics"
    }

    fn version(&self) -> &'static str {
        "1.0.0"
    }

    fn update(&mut self, _ctx: &mut PluginContext<'_>, dt: f32) -> Result<()> {
        let dt_ms = dt * 1000.0;
        if self.frame_hist.len() == self.frame_capacity {
            self.frame_hist.remove(0);
        }
        self.frame_hist.push(dt_ms);
        Ok(())
    }

    fn on_events(&mut self, _ctx: &mut PluginContext<'_>, events: &[GameEvent]) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        for event in events {
            if self.events.len() == self.event_capacity {
                self.events.pop_front();
            }
            self.events.push_back(event.clone());
        }
        Ok(())
    }

    fn shutdown(&mut self, _ctx: &mut PluginContext<'_>) -> Result<()> {
        self.events.clear();
        self.frame_hist.clear();
        self.particle_budget = None;
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
