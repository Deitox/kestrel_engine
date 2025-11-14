use crate::ecs::AnimationTime;
use bevy_ecs::prelude::Entity;
use egui::{self, Ui};

/// Lightweight summary for each animation track shown in the panel.
#[derive(Clone, Debug, Default)]
pub struct AnimationTrackSummary {
    pub label: String,
    pub key_count: usize,
    pub key_details: Vec<KeyframeDetail>,
}

#[derive(Clone, Debug, Default)]
pub struct KeyframeDetail {
    pub index: usize,
    pub time: Option<f32>,
    pub value_preview: Option<String>,
}

/// Snapshot of editor state passed into the panel each frame.
pub struct AnimationKeyframePanelState<'a> {
    pub animation_time: &'a AnimationTime,
    pub selected_entity: Option<Entity>,
    pub track_summaries: Vec<AnimationTrackSummary>,
}

#[derive(Default)]
pub struct AnimationKeyframePanel {
    open: bool,
    track_filter: String,
}

impl AnimationKeyframePanel {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    pub fn render_window(&mut self, ctx: &egui::Context, state: AnimationKeyframePanelState<'_>) {
        let mut open = self.open;
        egui::Window::new("Keyframe Editor (Milestone 5)")
            .open(&mut open)
            .default_width(480.0)
            .min_height(320.0)
            .show(ctx, |ui| {
                self.render_contents(ui, &state);
            });
        self.open = open;
    }

    fn render_contents(&mut self, ui: &mut Ui, state: &AnimationKeyframePanelState<'_>) {
        ui.heading("Keyframe Timeline");
        ui.label("Experimental tooling placeholder. See docs/animation_milestone_5_plan.md for full spec.");
        if let Some(entity) = state.selected_entity {
            ui.label(format!("Selected entity: {}", entity.index()));
        } else {
            ui.label("No entity selected.");
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Track filter:");
            ui.text_edit_singleline(&mut self.track_filter);
            if ui.button("Clear").clicked() {
                self.track_filter.clear();
            }
        });
        ui.separator();
        if state.track_summaries.is_empty() {
            ui.label("Tracks will appear here once the editor wiring is complete.");
            ui.small("Milestone task: bind Sprite/Transform/Skeletal tracks and render editable keys.");
        } else {
            egui::ScrollArea::vertical().show(ui, |ui| {
                for summary in &state.track_summaries {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(&summary.label);
                            ui.small(format!("{} keys", summary.key_count));
                        });
                        if summary.key_details.is_empty() {
                            ui.small("No key details available yet.");
                        } else {
                            egui::Grid::new(format!("key_grid_{}", summary.label)).striped(true).show(
                                ui,
                                |ui| {
                                    ui.label("Index");
                                    ui.label("Time");
                                    ui.label("Value");
                                    ui.end_row();
                                    for detail in &summary.key_details {
                                        ui.label(format!("#{}", detail.index));
                                        ui.label(
                                            detail
                                                .time
                                                .map(|t| format!("{t:.3}s"))
                                                .unwrap_or_else(|| "-".to_string()),
                                        );
                                        ui.label(
                                            detail
                                                .value_preview
                                                .as_deref()
                                                .unwrap_or("value preview pending"),
                                        );
                                        ui.end_row();
                                    }
                                },
                            );
                        }
                    });
                }
            });
        }
        ui.separator();
        ui.label(format!(
            "Animation time: scale {:.2}, paused {}, fixed_step {:?}",
            state.animation_time.scale, state.animation_time.paused, state.animation_time.fixed_step
        ));
        if !state.animation_time.group_scales.is_empty() {
            egui::CollapsingHeader::new("Group Scales").show(ui, |ui| {
                for (group, scale) in state.animation_time.group_scales.iter() {
                    ui.label(format!("{group}: {scale:.2}x"));
                }
            });
        }
        ui.separator();
        ui.label("Next steps:");
        ui.label("* Wire actual track data + inspector bindings.");
        ui.label("* Implement add/move/delete interactions per the spec.");
        ui.label("* Hook undo/redo + watcher persistence.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_count_matches_summary_vector() {
        let animation_time = AnimationTime::default();
        let state = AnimationKeyframePanelState {
            animation_time: &animation_time,
            selected_entity: None,
            track_summaries: vec![
                AnimationTrackSummary {
                    label: "Sprite/Translation".to_string(),
                    key_count: 12,
                    key_details: Vec::new(),
                },
                AnimationTrackSummary {
                    label: "Sprite/Rotation".to_string(),
                    key_count: 4,
                    key_details: Vec::new(),
                },
            ],
        };
        assert_eq!(state.track_summaries.len(), 2);
    }

    #[test]
    fn summary_with_details_keeps_metadata() {
        let summary = AnimationTrackSummary {
            label: "Transform Clip".to_string(),
            key_count: 2,
            key_details: vec![
                KeyframeDetail {
                    index: 0,
                    time: Some(0.0),
                    value_preview: Some("Translation (0,0)".to_string()),
                },
                KeyframeDetail {
                    index: 1,
                    time: Some(1.0),
                    value_preview: Some("Rotation 90deg".to_string()),
                },
            ],
        };
        assert_eq!(summary.key_details.len(), 2);
        assert_eq!(summary.key_details[1].time, Some(1.0));
    }
}
