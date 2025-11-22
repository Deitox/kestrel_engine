use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use super::{editor_ui, App, FrameTimingSample};
#[cfg(feature = "alloc_profiler")]
use crate::alloc_profiler;
use crate::assets::AssetManager;
use crate::environment::EnvironmentRegistry;
use crate::mesh_registry::MeshRegistry;
use crate::prefab::PrefabLibrary;
use crate::renderer::GpuPassTiming;
use egui_plot as eplot;

#[derive(Default)]
pub(super) struct TelemetryCache {
    mesh_keys: VersionedTelemetry<Arc<[String]>>,
    mesh_subsets: VersionedTelemetry<Arc<HashMap<String, Arc<[editor_ui::MeshSubsetEntry]>>>>,
    environment_options: VersionedTelemetry<Arc<[(String, String)]>>,
    prefab_entries: VersionedTelemetry<Arc<[editor_ui::PrefabShelfEntry]>>,
    clip_keys: VersionedTelemetry<Arc<[String]>>,
    clip_assets: VersionedTelemetry<Arc<HashMap<String, editor_ui::ClipAssetSummary>>>,
    skeleton_keys: VersionedTelemetry<Arc<[String]>>,
    skeleton_assets: VersionedTelemetry<Arc<HashMap<String, editor_ui::SkeletonAssetSummary>>>,
    atlas_keys: VersionedTelemetry<Arc<[String]>>,
    atlas_assets: VersionedTelemetry<Arc<HashMap<String, editor_ui::AtlasAssetSummary>>>,
}

impl TelemetryCache {
    pub(super) fn mesh_keys(&mut self, registry: &MeshRegistry) -> Arc<[String]> {
        self.mesh_keys.get_or_update(registry.version(), || {
            let mut keys = registry.keys().map(|k| k.to_string()).collect::<Vec<_>>();
            keys.sort();
            Arc::from(keys.into_boxed_slice())
        })
    }

    pub(super) fn mesh_subsets(
        &mut self,
        registry: &MeshRegistry,
    ) -> Arc<HashMap<String, Arc<[editor_ui::MeshSubsetEntry]>>> {
        self.mesh_subsets.get_or_update(registry.version(), || {
            let map: HashMap<String, Arc<[editor_ui::MeshSubsetEntry]>> = registry
                .keys()
                .filter_map(|key| {
                    registry.mesh_subsets(key).map(|subsets| {
                        let entries: Vec<editor_ui::MeshSubsetEntry> = subsets
                            .iter()
                            .map(|subset| editor_ui::MeshSubsetEntry {
                                name: subset.name.clone(),
                                index_offset: subset.index_offset,
                                index_count: subset.index_count,
                                material: subset.material.clone(),
                            })
                            .collect();
                        (key.to_string(), Arc::from(entries.into_boxed_slice()))
                    })
                })
                .collect();
            Arc::new(map)
        })
    }

    pub(super) fn environment_options(&mut self, registry: &EnvironmentRegistry) -> Arc<[(String, String)]> {
        self.environment_options.get_or_update(registry.version(), || {
            let mut options = registry
                .keys()
                .filter_map(|key| {
                    registry.definition(key).map(|definition| (key.clone(), definition.label().to_string()))
                })
                .collect::<Vec<_>>();
            options.sort_by(|a, b| a.1.cmp(&b.1));
            Arc::from(options.into_boxed_slice())
        })
    }

    pub(super) fn prefab_entries(&mut self, library: &PrefabLibrary) -> Arc<[editor_ui::PrefabShelfEntry]> {
        self.prefab_entries.get_or_update(library.version(), || {
            library
                .entries()
                .iter()
                .map(|entry| {
                    let relative = entry
                        .path
                        .strip_prefix(library.root())
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| entry.path.display().to_string());
                    editor_ui::PrefabShelfEntry {
                        name: entry.name.clone(),
                        format: entry.format,
                        path_display: relative,
                    }
                })
                .collect::<Vec<_>>()
                .into_boxed_slice()
                .into()
        })
    }

    pub(super) fn clip_assets(
        &mut self,
        assets: &AssetManager,
    ) -> (Arc<[String]>, Arc<HashMap<String, editor_ui::ClipAssetSummary>>) {
        let version = assets.revision();
        let keys = self.clip_keys.get_or_update(version, || Arc::from(assets.clip_keys().into_boxed_slice()));
        let summaries = self.clip_assets.get_or_update(version, || {
            let map: HashMap<String, editor_ui::ClipAssetSummary> = keys
                .iter()
                .map(|key| {
                    let source = assets.clip_source(key).map(|s| s.to_string());
                    let markers = assets
                        .clip(key)
                        .map(|clip| {
                            let mut markers = Vec::new();
                            if let Some(track) = clip.translation.as_ref() {
                                markers.extend(track.keyframes.iter().map(|kf| kf.time));
                            }
                            if let Some(track) = clip.rotation.as_ref() {
                                markers.extend(track.keyframes.iter().map(|kf| kf.time));
                            }
                            if let Some(track) = clip.scale.as_ref() {
                                markers.extend(track.keyframes.iter().map(|kf| kf.time));
                            }
                            if let Some(track) = clip.tint.as_ref() {
                                markers.extend(track.keyframes.iter().map(|kf| kf.time));
                            }
                            markers.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
                            markers.dedup_by(|a, b| (*a - *b).abs() <= 1e-4);
                            Arc::from(markers.into_boxed_slice())
                        })
                        .unwrap_or_else(|| Arc::from(Vec::<f32>::new().into_boxed_slice()));
                    (key.to_string(), editor_ui::ClipAssetSummary { source, keyframe_markers: markers })
                })
                .collect();
            Arc::new(map)
        });
        (keys, summaries)
    }

    pub(super) fn skeleton_assets(
        &mut self,
        assets: &AssetManager,
    ) -> (Arc<[String]>, Arc<HashMap<String, editor_ui::SkeletonAssetSummary>>) {
        let version = assets.revision();
        let keys = self
            .skeleton_keys
            .get_or_update(version, || Arc::from(assets.skeleton_keys().into_boxed_slice()));
        let map = self.skeleton_assets.get_or_update(version, || {
            let summaries: HashMap<String, editor_ui::SkeletonAssetSummary> = keys
                .iter()
                .map(|key| {
                    let clip_keys =
                        assets.skeletal_clip_keys_for(key).map(|keys| keys.to_vec()).unwrap_or_default();
                    let source = assets.skeleton_source(key).map(|s| s.to_string());
                    (
                        key.to_string(),
                        editor_ui::SkeletonAssetSummary {
                            source,
                            clip_keys: Arc::from(clip_keys.into_boxed_slice()),
                        },
                    )
                })
                .collect();
            Arc::new(summaries)
        });
        (keys, map)
    }

    pub(super) fn atlas_assets(
        &mut self,
        assets: &AssetManager,
    ) -> (Arc<[String]>, Arc<HashMap<String, editor_ui::AtlasAssetSummary>>) {
        let version = assets.revision();
        let keys =
            self.atlas_keys.get_or_update(version, || Arc::from(assets.atlas_keys().into_boxed_slice()));
        let map = self.atlas_assets.get_or_update(version, || {
            let summaries: HashMap<String, editor_ui::AtlasAssetSummary> = keys
                .iter()
                .map(|key| {
                    let mut timelines = assets.atlas_timeline_names(key);
                    timelines.sort();
                    timelines.dedup();
                    let source = assets.atlas_source(key).map(|s| s.to_string());
                    (
                        key.to_string(),
                        editor_ui::AtlasAssetSummary {
                            source,
                            timeline_names: Arc::from(timelines.into_boxed_slice()),
                        },
                    )
                })
                .collect();
            Arc::new(summaries)
        });
        (keys, map)
    }
}

#[derive(Clone)]
pub(super) struct GpuTimingFrame {
    pub(super) frame_index: u64,
    pub(super) timings: Vec<GpuPassTiming>,
}

pub(super) struct FrameProfiler {
    history: VecDeque<FrameTimingSample>,
    capacity: usize,
}

impl FrameProfiler {
    pub(super) fn new(capacity: usize) -> Self {
        Self { history: VecDeque::with_capacity(capacity), capacity: capacity.max(1) }
    }

    pub(super) fn push(&mut self, sample: FrameTimingSample) {
        if self.history.len() == self.capacity {
            self.history.pop_front();
        }
        self.history.push_back(sample);
    }

    pub(super) fn latest(&self) -> Option<FrameTimingSample> {
        self.history.back().copied()
    }
}

struct VersionedTelemetry<T> {
    version: Option<u64>,
    data: Option<T>,
}

impl<T> Default for VersionedTelemetry<T> {
    fn default() -> Self {
        Self { version: None, data: None }
    }
}

impl<T: Clone> VersionedTelemetry<T> {
    fn get_or_update<F>(&mut self, version: u64, rebuild: F) -> T
    where
        F: FnOnce() -> T,
    {
        if let (Some(current_version), Some(data)) = (&self.version, &self.data) {
            if *current_version == version {
                return data.clone();
            }
        }
        let arc = rebuild();
        self.version = Some(version);
        self.data = Some(arc.clone());
        arc
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct FrameBudgetSnapshot {
    pub(super) timing: Option<FrameTimingSample>,
    #[cfg(feature = "alloc_profiler")]
    pub(super) alloc_delta: Option<alloc_profiler::AllocationDelta>,
}

impl App {
    pub(crate) fn record_frame_timing_sample(&self, sample: FrameTimingSample) {
        self.with_editor_ui_state_mut(|state| state.frame_profiler.push(sample));
    }

    pub(crate) fn latest_frame_timing(&self) -> Option<FrameTimingSample> {
        self.editor_ui_state().frame_profiler.latest()
    }

    pub(crate) fn update_gpu_timing_snapshots(&self, timings: Vec<GpuPassTiming>) {
        if timings.is_empty() {
            return;
        }
        let arc_timings = Arc::from(timings.clone().into_boxed_slice());
        self.with_editor_ui_state_mut(|state| {
            state.gpu_timings = Arc::clone(&arc_timings);
            state.gpu_frame_counter = state.gpu_frame_counter.saturating_add(1);
            state
                .gpu_timing_history
                .push_back(GpuTimingFrame { frame_index: state.gpu_frame_counter, timings });
            while state.gpu_timing_history.len() > state.gpu_timing_history_capacity {
                state.gpu_timing_history.pop_front();
            }
        });
    }

    pub(super) fn frame_plot_points_arc(&mut self) -> Arc<[eplot::PlotPoint]> {
        let revision = self.analytics_plugin().map(|plugin| plugin.frame_history_revision()).unwrap_or(0);
        let needs_refresh = {
            let state = self.editor_ui_state();
            state.frame_plot_revision != revision
        };
        if needs_refresh {
            let new_arc = if let Some(plugin) = self.analytics_plugin() {
                let history = plugin.frame_history();
                let mut data = Vec::with_capacity(history.len());
                for (idx, value) in history.iter().enumerate() {
                    data.push(eplot::PlotPoint::new(idx as f64, *value as f64));
                }
                Arc::from(data.into_boxed_slice())
            } else {
                Arc::from(Vec::<eplot::PlotPoint>::new().into_boxed_slice())
            };
            self.with_editor_ui_state_mut(|state| {
                state.frame_plot_revision = revision;
                state.frame_plot_points = Arc::clone(&new_arc);
            });
            return new_arc;
        }
        let state = self.editor_ui_state();
        Arc::clone(&state.frame_plot_points)
    }

    pub(super) fn capture_frame_budget_snapshot(&self) -> FrameBudgetSnapshot {
        FrameBudgetSnapshot {
            timing: self.latest_frame_timing(),
            #[cfg(feature = "alloc_profiler")]
            alloc_delta: self.analytics_plugin().and_then(|plugin| plugin.allocation_delta()),
        }
    }

    pub(super) fn frame_budget_snapshot_view(
        snapshot: &FrameBudgetSnapshot,
    ) -> editor_ui::FrameBudgetSnapshotView {
        editor_ui::FrameBudgetSnapshotView {
            timing: snapshot.timing,
            #[cfg(feature = "alloc_profiler")]
            alloc_delta: snapshot.alloc_delta,
        }
    }

    pub(super) fn frame_budget_delta_message(&self) -> Option<String> {
        let (baseline_snapshot, comparison_snapshot) = {
            let state = self.editor_ui_state();
            (state.frame_budget_idle_snapshot, state.frame_budget_panel_snapshot)
        };
        let baseline = baseline_snapshot?;
        let comparison = comparison_snapshot?;
        let idle = baseline.timing?;
        let panel = comparison.timing?;
        let update_delta = panel.update_ms - idle.update_ms;
        let ui_delta = panel.ui_ms - idle.ui_ms;
        #[cfg(feature = "alloc_profiler")]
        let alloc_note =
            if let (Some(idle_alloc), Some(panel_alloc)) = (baseline.alloc_delta, comparison.alloc_delta) {
                let diff = panel_alloc.net_bytes() - idle_alloc.net_bytes();
                format!(", delta_alloc={:+} B", diff)
            } else {
                String::new()
            };
        #[cfg(not(feature = "alloc_profiler"))]
        let alloc_note = String::new();
        Some(format!(
            "Frame budget delta: delta_update={:+.2} ms, delta_ui={:+.2} ms{alloc_note}",
            update_delta, ui_delta
        ))
    }

    pub(super) fn handle_frame_budget_action(&mut self, action: Option<editor_ui::FrameBudgetAction>) {
        use editor_ui::FrameBudgetAction;
        let Some(action) = action else {
            return;
        };
        match action {
            FrameBudgetAction::CaptureIdle => {
                let snapshot = self.capture_frame_budget_snapshot();
                self.with_editor_ui_state_mut(|state| {
                    state.frame_budget_idle_snapshot = Some(snapshot);
                    state.frame_budget_status = Some(
                        "Idle baseline captured. Toggle panels, then capture the panel snapshot.".to_string(),
                    );
                });
            }
            FrameBudgetAction::CapturePanel => {
                let snapshot = self.capture_frame_budget_snapshot();
                self.with_editor_ui_state_mut(|state| {
                    state.frame_budget_panel_snapshot = Some(snapshot);
                });
                let status = self.frame_budget_delta_message().or_else(|| {
                    Some(
                        "Panel snapshot captured. Capture an idle baseline first for delta comparisons."
                            .to_string(),
                    )
                });
                self.with_editor_ui_state_mut(|state| state.frame_budget_status = status);
            }
            FrameBudgetAction::Clear => {
                self.with_editor_ui_state_mut(|state| {
                    state.frame_budget_idle_snapshot = None;
                    state.frame_budget_panel_snapshot = None;
                    state.frame_budget_status = Some("Cleared frame budget snapshots.".to_string());
                });
            }
        }
    }
}
