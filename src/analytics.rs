#[cfg(feature = "alloc_profiler")]
use crate::alloc_profiler::AllocationDelta;
use crate::animation_validation::AnimationValidationEvent;
use crate::ecs::{ParticleBudgetMetrics, SpatialMetrics};
use crate::events::GameEvent;
use crate::plugins::{
    CapabilityViolationLog, EnginePlugin, PluginAssetReadbackEvent, PluginCapabilityEvent, PluginContext,
    PluginWatchdogEvent,
};
use crate::renderer::{GpuPassTiming, LightClusterMetrics};
use anyhow::Result;
use serde::Serialize;
use std::any::Any;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct AnimationBudgetSample {
    pub sprite_eval_ms: f32,
    pub sprite_pack_ms: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sprite_upload_ms: Option<f32>,
    pub transform_eval_ms: f32,
    pub skeletal_eval_ms: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub palette_upload_ms: Option<f32>,
    pub sprite_animators: u32,
    pub transform_clip_count: usize,
    pub skeletal_instance_count: usize,
    pub skeletal_bone_count: usize,
    pub palette_upload_calls: u32,
    pub palette_uploaded_joints: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct KeyframeEditorUsageSnapshot {
    pub panel_open_count: u64,
    pub panel_close_count: u64,
    pub scrub_count: u64,
    pub insert_count: u64,
    pub delete_count: u64,
    pub delete_key_total: u64,
    pub update_count: u64,
    pub update_time_edits: u64,
    pub update_value_edits: u64,
    pub adjust_count: u64,
    pub adjust_time_edits: u64,
    pub adjust_value_edits: u64,
    pub undo_count: u64,
    pub redo_count: u64,
}

impl KeyframeEditorUsageSnapshot {
    fn register(&mut self, event: &KeyframeEditorEventKind) {
        match event {
            KeyframeEditorEventKind::PanelOpened => self.panel_open_count += 1,
            KeyframeEditorEventKind::PanelClosed => self.panel_close_count += 1,
            KeyframeEditorEventKind::Scrub { .. } => self.scrub_count += 1,
            KeyframeEditorEventKind::InsertKey { .. } => self.insert_count += 1,
            KeyframeEditorEventKind::DeleteKeys { count, .. } => {
                self.delete_count += 1;
                self.delete_key_total += *count as u64;
            }
            KeyframeEditorEventKind::UpdateKey { changed_time, changed_value, .. } => {
                self.update_count += 1;
                if *changed_time {
                    self.update_time_edits += 1;
                }
                if *changed_value {
                    self.update_value_edits += 1;
                }
            }
            KeyframeEditorEventKind::AdjustKeys { time_delta, value_delta, .. } => {
                self.adjust_count += 1;
                if *time_delta {
                    self.adjust_time_edits += 1;
                }
                if *value_delta {
                    self.adjust_value_edits += 1;
                }
            }
            KeyframeEditorEventKind::Undo => self.undo_count += 1,
            KeyframeEditorEventKind::Redo => self.redo_count += 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyframeEditorTrackKind {
    SpriteTimeline,
    Translation,
    Rotation,
    Scale,
    Tint,
    Unknown,
}

#[derive(Clone, Copy, Debug)]
pub enum KeyframeEditorEventKind {
    PanelOpened,
    PanelClosed,
    Scrub { track: KeyframeEditorTrackKind },
    InsertKey { track: KeyframeEditorTrackKind },
    DeleteKeys { track: KeyframeEditorTrackKind, count: usize },
    UpdateKey { track: KeyframeEditorTrackKind, changed_time: bool, changed_value: bool },
    AdjustKeys { track: KeyframeEditorTrackKind, count: usize, time_delta: bool, value_delta: bool },
    Undo,
    Redo,
}

#[derive(Clone, Copy, Debug)]
pub struct KeyframeEditorEvent {
    pub timestamp: Instant,
    pub kind: KeyframeEditorEventKind,
}

pub struct AnalyticsPlugin {
    frame_hist: Vec<f32>,
    frame_capacity: usize,
    frame_hist_revision: u64,
    events: VecDeque<GameEvent>,
    event_capacity: usize,
    events_snapshot: Option<Arc<[GameEvent]>>,
    particle_budget: Option<ParticleBudgetMetrics>,
    spatial_metrics: Option<SpatialMetrics>,
    light_cluster_metrics: Option<LightClusterMetrics>,
    gpu_capacity: usize,
    gpu_timings: BTreeMap<&'static str, VecDeque<f32>>,
    gpu_timings_snapshot: Option<Arc<HashMap<&'static str, Vec<f32>>>>,
    plugin_capability_metrics: Arc<HashMap<String, CapabilityViolationLog>>,
    plugin_capability_events: VecDeque<PluginCapabilityEvent>,
    plugin_asset_readbacks: VecDeque<PluginAssetReadbackEvent>,
    plugin_watchdog_events: VecDeque<PluginWatchdogEvent>,
    plugin_capability_snapshot: Option<Arc<[PluginCapabilityEvent]>>,
    plugin_asset_readback_snapshot: Option<Arc<[PluginAssetReadbackEvent]>>,
    plugin_watchdog_snapshot: Option<Arc<[PluginWatchdogEvent]>>,
    animation_validation_events: VecDeque<AnimationValidationEvent>,
    animation_validation_snapshot: Option<Arc<[AnimationValidationEvent]>>,
    animation_budget_sample: Option<AnimationBudgetSample>,
    keyframe_editor_usage: KeyframeEditorUsageSnapshot,
    keyframe_editor_events: VecDeque<KeyframeEditorEvent>,
    keyframe_events_snapshot: Option<Arc<[KeyframeEditorEvent]>>,
    #[cfg(feature = "alloc_profiler")]
    allocation_delta: Option<AllocationDelta>,
}

const SECURITY_EVENT_CAPACITY: usize = 64;
const KEYFRAME_EVENT_CAPACITY: usize = 32;

impl AnalyticsPlugin {
    pub fn new(frame_capacity: usize, event_capacity: usize) -> Self {
        Self {
            frame_hist: Vec::with_capacity(frame_capacity.min(1_024)),
            frame_capacity: frame_capacity.max(1),
            frame_hist_revision: 0,
            events: VecDeque::with_capacity(event_capacity.min(1_024)),
            event_capacity: event_capacity.max(1),
            events_snapshot: None,
            particle_budget: None,
            spatial_metrics: None,
            light_cluster_metrics: None,
            gpu_capacity: 120,
            gpu_timings: BTreeMap::new(),
            gpu_timings_snapshot: None,
            plugin_capability_metrics: Arc::new(HashMap::new()),
            plugin_capability_events: VecDeque::with_capacity(SECURITY_EVENT_CAPACITY),
            plugin_asset_readbacks: VecDeque::with_capacity(32),
            plugin_watchdog_events: VecDeque::with_capacity(32),
            plugin_capability_snapshot: None,
            plugin_asset_readback_snapshot: None,
            plugin_watchdog_snapshot: None,
            animation_validation_events: VecDeque::with_capacity(SECURITY_EVENT_CAPACITY),
            animation_validation_snapshot: None,
            animation_budget_sample: None,
            keyframe_editor_usage: KeyframeEditorUsageSnapshot::default(),
            keyframe_editor_events: VecDeque::with_capacity(KEYFRAME_EVENT_CAPACITY),
            keyframe_events_snapshot: None,
            #[cfg(feature = "alloc_profiler")]
            allocation_delta: None,
        }
    }

    pub fn frame_history(&self) -> &[f32] {
        &self.frame_hist
    }

    pub fn frame_history_revision(&self) -> u64 {
        self.frame_hist_revision
    }

    pub fn recent_events(&self) -> impl Iterator<Item = &GameEvent> {
        self.events.iter()
    }

    pub fn recent_events_snapshot(&mut self) -> Arc<[GameEvent]> {
        if let Some(cache) = &self.events_snapshot {
            return Arc::clone(cache);
        }
        let data = self.events.iter().cloned().collect::<Vec<_>>();
        let arc = Arc::from(data.into_boxed_slice());
        self.events_snapshot = Some(Arc::clone(&arc));
        arc
    }

    pub fn clear_frame_history(&mut self) {
        self.frame_hist.clear();
        self.frame_hist_revision = self.frame_hist_revision.wrapping_add(1);
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

    pub fn record_light_cluster_metrics(&mut self, metrics: LightClusterMetrics) {
        self.light_cluster_metrics = Some(metrics);
    }

    pub fn light_cluster_metrics(&self) -> Option<LightClusterMetrics> {
        self.light_cluster_metrics
    }

    pub fn record_gpu_timings(&mut self, timings: &[GpuPassTiming]) {
        if timings.is_empty() {
            return;
        }
        self.gpu_timings_snapshot = None;
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

    pub fn record_plugin_capability_metrics(
        &mut self,
        metrics: Arc<HashMap<String, CapabilityViolationLog>>,
    ) {
        self.plugin_capability_metrics = metrics;
    }

    pub fn plugin_capability_metrics(&self) -> Arc<HashMap<String, CapabilityViolationLog>> {
        Arc::clone(&self.plugin_capability_metrics)
    }

    #[cfg(feature = "alloc_profiler")]
    pub fn record_allocation_delta(&mut self, delta: AllocationDelta) {
        self.allocation_delta = Some(delta);
    }

    #[cfg(feature = "alloc_profiler")]
    pub fn allocation_delta(&self) -> Option<AllocationDelta> {
        self.allocation_delta
    }

    pub fn record_plugin_capability_events(
        &mut self,
        events: impl IntoIterator<Item = PluginCapabilityEvent>,
    ) {
        for event in events {
            self.plugin_capability_events.push_front(event);
            if self.plugin_capability_events.len() > SECURITY_EVENT_CAPACITY {
                self.plugin_capability_events.pop_back();
            }
        }
        self.plugin_capability_snapshot = None;
    }

    pub fn gpu_timings_snapshot(&mut self) -> Arc<HashMap<&'static str, Vec<f32>>> {
        if let Some(cache) = &self.gpu_timings_snapshot {
            return Arc::clone(cache);
        }
        let map: HashMap<&'static str, Vec<f32>> = self
            .gpu_timings
            .iter()
            .map(|(label, samples)| (*label, samples.iter().copied().collect()))
            .collect();
        let arc = Arc::new(map);
        self.gpu_timings_snapshot = Some(Arc::clone(&arc));
        arc
    }

    pub fn plugin_capability_events_arc(&mut self) -> Arc<[PluginCapabilityEvent]> {
        if let Some(cache) = &self.plugin_capability_snapshot {
            return Arc::clone(cache);
        }
        let data = self.plugin_capability_events.iter().cloned().collect::<Vec<_>>();
        let arc = Arc::from(data.into_boxed_slice());
        self.plugin_capability_snapshot = Some(Arc::clone(&arc));
        arc
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
        self.plugin_asset_readback_snapshot = None;
    }

    pub fn plugin_asset_readbacks_arc(&mut self) -> Arc<[PluginAssetReadbackEvent]> {
        if let Some(cache) = &self.plugin_asset_readback_snapshot {
            return Arc::clone(cache);
        }
        let data = self.plugin_asset_readbacks.iter().cloned().collect::<Vec<_>>();
        let arc = Arc::from(data.into_boxed_slice());
        self.plugin_asset_readback_snapshot = Some(Arc::clone(&arc));
        arc
    }

    pub fn record_plugin_watchdog_events(&mut self, events: impl IntoIterator<Item = PluginWatchdogEvent>) {
        for event in events {
            self.plugin_watchdog_events.push_front(event);
            if self.plugin_watchdog_events.len() > SECURITY_EVENT_CAPACITY {
                self.plugin_watchdog_events.pop_back();
            }
        }
        self.plugin_watchdog_snapshot = None;
    }

    pub fn plugin_watchdog_events_arc(&mut self) -> Arc<[PluginWatchdogEvent]> {
        if let Some(cache) = &self.plugin_watchdog_snapshot {
            return Arc::clone(cache);
        }
        let data = self.plugin_watchdog_events.iter().cloned().collect::<Vec<_>>();
        let arc = Arc::from(data.into_boxed_slice());
        self.plugin_watchdog_snapshot = Some(Arc::clone(&arc));
        arc
    }

    pub fn record_animation_validation_events(
        &mut self,
        events: impl IntoIterator<Item = AnimationValidationEvent>,
    ) {
        for event in events {
            self.animation_validation_events.push_front(event);
            if self.animation_validation_events.len() > SECURITY_EVENT_CAPACITY {
                self.animation_validation_events.pop_back();
            }
        }
        self.animation_validation_snapshot = None;
    }

    pub fn animation_validation_events_arc(&mut self) -> Arc<[AnimationValidationEvent]> {
        if let Some(cache) = &self.animation_validation_snapshot {
            return Arc::clone(cache);
        }
        let data = self.animation_validation_events.iter().cloned().collect::<Vec<_>>();
        let arc = Arc::from(data.into_boxed_slice());
        self.animation_validation_snapshot = Some(Arc::clone(&arc));
        arc
    }

    pub fn record_animation_budget_sample(&mut self, sample: AnimationBudgetSample) {
        self.animation_budget_sample = Some(sample);
    }

    pub fn animation_budget_sample(&self) -> Option<AnimationBudgetSample> {
        self.animation_budget_sample
    }

    pub fn record_keyframe_editor_event(&mut self, kind: KeyframeEditorEventKind) {
        self.keyframe_editor_usage.register(&kind);
        self.keyframe_editor_events.push_front(KeyframeEditorEvent { timestamp: Instant::now(), kind });
        if self.keyframe_editor_events.len() > KEYFRAME_EVENT_CAPACITY {
            self.keyframe_editor_events.pop_back();
        }
        self.keyframe_events_snapshot = None;
    }

    pub fn keyframe_editor_usage(&self) -> KeyframeEditorUsageSnapshot {
        self.keyframe_editor_usage
    }

    pub fn keyframe_editor_events_arc(&mut self) -> Arc<[KeyframeEditorEvent]> {
        if let Some(cache) = &self.keyframe_events_snapshot {
            return Arc::clone(cache);
        }
        let data = self.keyframe_editor_events.iter().cloned().collect::<Vec<_>>();
        let arc = Arc::from(data.into_boxed_slice());
        self.keyframe_events_snapshot = Some(Arc::clone(&arc));
        arc
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
        self.frame_hist_revision = self.frame_hist_revision.wrapping_add(1);
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
        self.events_snapshot = None;
        Ok(())
    }

    fn shutdown(&mut self, _ctx: &mut PluginContext<'_>) -> Result<()> {
        self.events.clear();
        self.events_snapshot = None;
        self.frame_hist_revision = self.frame_hist_revision.wrapping_add(1);
        self.frame_hist.clear();
        self.particle_budget = None;
        self.spatial_metrics = None;
        self.light_cluster_metrics = None;
        self.gpu_timings.clear();
        self.plugin_capability_events.clear();
        self.plugin_asset_readbacks.clear();
        self.plugin_watchdog_events.clear();
        self.plugin_capability_snapshot = None;
        self.plugin_asset_readback_snapshot = None;
        self.plugin_watchdog_snapshot = None;
        self.animation_validation_events.clear();
        self.animation_budget_sample = None;
        self.plugin_capability_metrics = Arc::new(HashMap::new());
        #[cfg(feature = "alloc_profiler")]
        {
            self.allocation_delta = None;
        }
        self.keyframe_editor_events.clear();
        self.keyframe_editor_usage = KeyframeEditorUsageSnapshot::default();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation_validation::{AnimationValidationEvent, AnimationValidationSeverity};
    use crate::plugins::{
        PluginAssetReadbackEvent, PluginCapability, PluginCapabilityEvent, PluginWatchdogEvent,
    };
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::SystemTime;

    #[test]
    fn animation_validation_events_recorded() {
        let mut analytics = AnalyticsPlugin::default();
        analytics.record_animation_validation_events(vec![AnimationValidationEvent {
            severity: AnimationValidationSeverity::Warning,
            path: PathBuf::from("assets/animations/example.clip"),
            message: "Test warning".to_string(),
        }]);
        let events = analytics.animation_validation_events_arc();
        assert_eq!(events.len(), 1);
        assert!(events[0].message.contains("Test warning"));
    }

    #[test]
    fn keyframe_editor_events_recorded() {
        let mut analytics = AnalyticsPlugin::default();
        analytics.record_keyframe_editor_event(KeyframeEditorEventKind::PanelOpened);
        analytics.record_keyframe_editor_event(KeyframeEditorEventKind::InsertKey {
            track: KeyframeEditorTrackKind::Translation,
        });
        analytics.record_keyframe_editor_event(KeyframeEditorEventKind::UpdateKey {
            track: KeyframeEditorTrackKind::Translation,
            changed_time: true,
            changed_value: false,
        });
        let usage = analytics.keyframe_editor_usage();
        assert_eq!(usage.panel_open_count, 1);
        assert_eq!(usage.insert_count, 1);
        assert_eq!(usage.update_count, 1);
        assert_eq!(usage.update_time_edits, 1);
        let events = analytics.keyframe_editor_events_arc();
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0].kind, KeyframeEditorEventKind::UpdateKey { changed_time: true, .. }));
    }

    #[test]
    fn plugin_event_snapshots_cache_until_mutated() {
        let mut analytics = AnalyticsPlugin::default();
        analytics.record_plugin_capability_events(vec![PluginCapabilityEvent {
            plugin: "example".to_string(),
            capability: PluginCapability::Assets,
            timestamp: SystemTime::now(),
        }]);
        let capability_first = analytics.plugin_capability_events_arc();
        let capability_second = analytics.plugin_capability_events_arc();
        assert!(Arc::ptr_eq(&capability_first, &capability_second));
        analytics.record_plugin_capability_events(vec![PluginCapabilityEvent {
            plugin: "example".to_string(),
            capability: PluginCapability::Ecs,
            timestamp: SystemTime::now(),
        }]);
        let capability_third = analytics.plugin_capability_events_arc();
        assert!(!Arc::ptr_eq(&capability_first, &capability_third));

        analytics.record_plugin_asset_readbacks(vec![PluginAssetReadbackEvent {
            plugin: "example".to_string(),
            kind: "blob_range".to_string(),
            target: "assets/blob.bin".to_string(),
            bytes: 128,
            duration_ms: 0.25,
            cache_hit: false,
            timestamp: SystemTime::now(),
        }]);
        let asset_first = analytics.plugin_asset_readbacks_arc();
        assert!(Arc::ptr_eq(&asset_first, &analytics.plugin_asset_readbacks_arc()));
        analytics.record_plugin_asset_readbacks(vec![PluginAssetReadbackEvent {
            plugin: "example".to_string(),
            kind: "blob_range".to_string(),
            target: "assets/blob.bin".to_string(),
            bytes: 64,
            duration_ms: 0.1,
            cache_hit: true,
            timestamp: SystemTime::now(),
        }]);
        assert!(!Arc::ptr_eq(&asset_first, &analytics.plugin_asset_readbacks_arc()));

        analytics.record_plugin_watchdog_events(vec![PluginWatchdogEvent {
            plugin: "example".to_string(),
            timestamp: SystemTime::now(),
            elapsed_ms: 12.0,
            reason: "timeout".to_string(),
            last_request: "asset_readback".to_string(),
        }]);
        let watchdog_first = analytics.plugin_watchdog_events_arc();
        assert!(Arc::ptr_eq(&watchdog_first, &analytics.plugin_watchdog_events_arc()));
        analytics.record_plugin_watchdog_events(vec![PluginWatchdogEvent {
            plugin: "example".to_string(),
            timestamp: SystemTime::now(),
            elapsed_ms: 1.0,
            reason: "panic".to_string(),
            last_request: "update".to_string(),
        }]);
        assert!(!Arc::ptr_eq(&watchdog_first, &analytics.plugin_watchdog_events_arc()));
    }
}
