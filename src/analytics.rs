use crate::ecs::{ParticleBudgetMetrics, SpatialMetrics};
use crate::events::GameEvent;
use crate::plugins::{
    CapabilityViolationLog, EnginePlugin, PluginAssetReadbackEvent, PluginContext, PluginWatchdogEvent,
};
use crate::renderer::GpuPassTiming;
use anyhow::Result;
use std::any::Any;
use std::collections::{BTreeMap, HashMap, VecDeque};

pub struct AnalyticsPlugin {
    frame_hist: Vec<f32>,
    frame_capacity: usize,
    events: VecDeque<GameEvent>,
    event_capacity: usize,
    particle_budget: Option<ParticleBudgetMetrics>,
    spatial_metrics: Option<SpatialMetrics>,
    gpu_capacity: usize,
    gpu_timings: BTreeMap<&'static str, VecDeque<f32>>,
    plugin_capability_metrics: HashMap<String, CapabilityViolationLog>,
    plugin_asset_readbacks: VecDeque<PluginAssetReadbackEvent>,
    plugin_watchdog_events: VecDeque<PluginWatchdogEvent>,
}

const SECURITY_EVENT_CAPACITY: usize = 64;

impl AnalyticsPlugin {
    pub fn new(frame_capacity: usize, event_capacity: usize) -> Self {
        Self {
            frame_hist: Vec::with_capacity(frame_capacity.min(1_024)),
            frame_capacity: frame_capacity.max(1),
            events: VecDeque::with_capacity(event_capacity.min(1_024)),
            event_capacity: event_capacity.max(1),
            particle_budget: None,
            spatial_metrics: None,
            gpu_capacity: 120,
            gpu_timings: BTreeMap::new(),
            plugin_capability_metrics: HashMap::new(),
            plugin_asset_readbacks: VecDeque::with_capacity(32),
            plugin_watchdog_events: VecDeque::with_capacity(32),
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

    pub fn record_spatial_metrics(&mut self, metrics: SpatialMetrics) {
        self.spatial_metrics = Some(metrics);
    }

    pub fn spatial_metrics(&self) -> Option<SpatialMetrics> {
        self.spatial_metrics
    }

    pub fn record_gpu_timings(&mut self, timings: &[GpuPassTiming]) {
        if timings.is_empty() {
            return;
        }
        for timing in timings {
            let entry = self
                .gpu_timings
                .entry(timing.label)
                .or_insert_with(|| VecDeque::with_capacity(self.gpu_capacity));
            if entry.len() == self.gpu_capacity {
                entry.pop_front();
            }
            entry.push_back(timing.duration_ms);
        }
    }

    pub fn gpu_pass_metric(&self, label: &'static str) -> Option<GpuPassMetric> {
        let samples = self.gpu_timings.get(label)?;
        if samples.is_empty() {
            return None;
        }
        let latest_ms = *samples.back().unwrap();
        let sum: f32 = samples.iter().sum();
        let avg = sum / samples.len() as f32;
        Some(GpuPassMetric { label, latest_ms, average_ms: avg, sample_count: samples.len() })
    }

    pub fn record_plugin_capability_metrics(&mut self, metrics: HashMap<String, CapabilityViolationLog>) {
        self.plugin_capability_metrics = metrics;
    }

    pub fn plugin_capability_metrics(&self) -> &HashMap<String, CapabilityViolationLog> {
        &self.plugin_capability_metrics
    }

    pub fn record_plugin_asset_readbacks(
        &mut self,
        events: impl IntoIterator<Item = PluginAssetReadbackEvent>,
    ) {
        for event in events {
            self.plugin_asset_readbacks.push_front(event);
            if self.plugin_asset_readbacks.len() > SECURITY_EVENT_CAPACITY {
                self.plugin_asset_readbacks.pop_back();
            }
        }
    }

    pub fn record_plugin_watchdog_events(&mut self, events: impl IntoIterator<Item = PluginWatchdogEvent>) {
        for event in events {
            self.plugin_watchdog_events.push_front(event);
            if self.plugin_watchdog_events.len() > SECURITY_EVENT_CAPACITY {
                self.plugin_watchdog_events.pop_back();
            }
        }
    }

    pub fn plugin_asset_readbacks(&self) -> Vec<PluginAssetReadbackEvent> {
        self.plugin_asset_readbacks.iter().cloned().collect()
    }

    pub fn plugin_watchdog_events(&self) -> Vec<PluginWatchdogEvent> {
        self.plugin_watchdog_events.iter().cloned().collect()
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
        self.spatial_metrics = None;
        self.gpu_timings.clear();
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GpuPassMetric {
    pub label: &'static str,
    pub latest_ms: f32,
    pub average_ms: f32,
    pub sample_count: usize,
}
